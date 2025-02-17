use kube::CustomResourceExt;

fn main() {
    print!(
        "{}",
        serde_yaml::to_string(&pg_db_controller::PostgresDatabase::crd()).unwrap()
    )
}
