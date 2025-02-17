use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use schemars::JsonSchema;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[kube(
    kind = "PostgresDatabase",
    group = "inf-k8s.net",
    version = "v1",
    namespaced
)]
#[kube(status = "DocumentStatus")]
pub struct PgDatabase {
    pub database_name: String,
    pub role_name: Option<String>,
    pub secret_name: String,
    pub secret_namespace: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct DocumentStatus {
    checksum: String,
}
