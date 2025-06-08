use base64::{prelude::BASE64_URL_SAFE, Engine};
use futures::StreamExt;
use k8s_openapi::{api::core::v1::Secret, ByteString};
use kube::{
    api::{ObjectMeta, PostParams},
    runtime::controller::{Action, Controller},
    Api, Client, Resource, ResourceExt,
};
use pg_db_controller::PostgresDatabase;
use rand::{rngs::StdRng, RngCore, SeedableRng};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{collections::BTreeMap, sync::Arc, time::Duration};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("sqlx error: {0}")]
    SqlxError(#[from] sqlx::Error),

    #[error("kube error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("serde error: {0}")]
    SerdeJsonError(#[from] serde_json::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

struct ControllerContext {
    pub db: PgPool,
    pub client: Client,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().init();

    let database_url = std::env::var("DATABASE_URL").unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await?;

    tracing::info!("connected to db: {pool:?}");

    let client = Client::try_default().await?;

    let pg_databases = Api::<PostgresDatabase>::all(client.clone());

    let ctx = ControllerContext { db: pool, client };

    Controller::new(pg_databases.clone(), Default::default())
        .run(reconcile, error_policy, Arc::new(ctx))
        .for_each(|_| futures::future::ready(()))
        .await;

    Ok(())
}

async fn reconcile(obj: Arc<PostgresDatabase>, ctx: Arc<ControllerContext>) -> Result<Action> {
    tracing::info!("reconcile request for {}", obj.name_any());
    let database_to_create = &obj.spec.database_name;

    let db_exists = sqlx::query!(
        "SELECT datname FROM pg_catalog.pg_database WHERE lower(datname) = lower($1);",
        database_to_create
    )
    .fetch_all(&ctx.db)
    .await?
    .len()
        == 1;

    // update existing secret if DB already exists, but we can't ever add password
    // we could reset the password for the role though
    let secrets = Api::<Secret>::namespaced(ctx.client.clone(), &obj.spec.secret_namespace);
    let pp = PostParams::default();

    if db_exists {
        let existing_secret = secrets.get(&obj.spec.secret_name).await;
        if existing_secret.is_ok() {
            tracing::info!("existing secret found, not creating new");
            return Ok(Action::requeue(Duration::from_secs(3600)));
        }

        let role_name = obj.spec.role_name.as_ref().unwrap_or(database_to_create);
        let mut rng = StdRng::from_os_rng();
        let mut password_bytes = [0; 32];

        rng.fill_bytes(&mut password_bytes);

        let password = BASE64_URL_SAFE.encode(password_bytes);
        tracing::info!("using password: {password}");

        sqlx::query(&format!(
            "ALTER ROLE {role_name} WITH PASSWORD '{password}'"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!(
            "ALTER DATABASE {database_to_create} OWNER TO {role_name}"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!("ALTER ROLE {role_name} WITH LOGIN"))
            .execute(&ctx.db)
            .await?;

        let connection_options = ctx.db.connect_options();
        let host = connection_options.get_host();
        let port = connection_options.get_port();

        let db_url =
            format!("postgresql://{role_name}:{password}@{host}:{port}/{database_to_create}");

        let mut data = BTreeMap::new();
        data.insert(
            "PGPASSWORD".to_string(),
            ByteString(password.clone().into_bytes()),
        );
        data.insert("DATABASE_URL".to_string(), ByteString(db_url.into_bytes()));
        data.insert(
            "PGHOST".to_string(),
            ByteString(host.to_owned().into_bytes()),
        );
        data.insert(
            "PGPORT".to_string(),
            ByteString(port.to_string().into_bytes()),
        );
        data.insert(
            "PGDATABASE".to_string(),
            ByteString(database_to_create.to_string().into_bytes()),
        );
        data.insert(
            "PGUSER".to_string(),
            ByteString(role_name.to_string().into_bytes()),
        );

        let secret = Secret {
            data: Some(data),
            immutable: Some(true),
            metadata: ObjectMeta {
                name: Some(obj.spec.secret_name.clone()),
                namespace: Some(obj.spec.secret_namespace.clone()),
                owner_references: Some(vec![obj.controller_owner_ref(&()).unwrap()]),
                ..Default::default()
            },
            ..Default::default()
        };

        secrets.create(&pp, &secret).await?;

        tracing::info!("secret updated: {}", obj.spec.secret_name);
    } else {
        sqlx::query(&format!("CREATE DATABASE {database_to_create}"))
            .execute(&ctx.db)
            .await?;

        let role_name = obj.spec.role_name.as_ref().unwrap_or(database_to_create);
        let mut rng = StdRng::from_os_rng();
        let mut password_bytes = [0; 32];

        rng.fill_bytes(&mut password_bytes);

        let password = BASE64_URL_SAFE.encode(password_bytes);
        tracing::info!("using password: {password}");

        sqlx::query(&format!(
            "CREATE ROLE {role_name} WITH PASSWORD '{password}'"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!(
            "ALTER DATABASE {database_to_create} OWNER TO {role_name}"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!("ALTER ROLE {role_name} WITH LOGIN"))
            .execute(&ctx.db)
            .await?;

        let connection_options = ctx.db.connect_options();
        let host = connection_options.get_host();
        let port = connection_options.get_port();

        let db_url =
            format!("postgresql://{role_name}:{password}@{host}:{port}/{database_to_create}");

        let mut data = BTreeMap::new();
        data.insert(
            "PGPASSWORD".to_string(),
            ByteString(password.clone().into_bytes()),
        );
        data.insert("DATABASE_URL".to_string(), ByteString(db_url.into_bytes()));
        data.insert(
            "PGHOST".to_string(),
            ByteString(host.to_owned().into_bytes()),
        );
        data.insert(
            "PGPORT".to_string(),
            ByteString(port.to_string().into_bytes()),
        );
        data.insert(
            "PGDATABASE".to_string(),
            ByteString(database_to_create.to_string().into_bytes()),
        );
        data.insert(
            "PGUSER".to_string(),
            ByteString(role_name.to_string().into_bytes()),
        );

        let secret = Secret {
            data: Some(data),
            immutable: Some(true),
            metadata: ObjectMeta {
                name: Some(obj.spec.secret_name.clone()),
                namespace: Some(obj.spec.secret_namespace.clone()),
                owner_references: Some(vec![obj.controller_owner_ref(&()).unwrap()]),
                ..Default::default()
            },
            ..Default::default()
        };

        // update existing secret if DB already exists, but we can't ever add password
        // we could reset the password for the role though
        let secrets = Api::<Secret>::namespaced(ctx.client.clone(), &obj.spec.secret_namespace);
        let pp = PostParams::default();

        secrets.create(&pp, &secret).await?;

        tracing::info!("secret created: {}", obj.spec.secret_name);
    }

    Ok(Action::requeue(Duration::from_secs(3600)))
}

fn error_policy(
    _object: Arc<PostgresDatabase>,
    err: &Error,
    _ctx: Arc<ControllerContext>,
) -> Action {
    tracing::error!("error in reconcile: {err}");
    Action::requeue(Duration::from_secs(3600))
}
