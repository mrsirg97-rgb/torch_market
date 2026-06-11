# Secret Manager entries Cloud Run injects as env vars.
#
# The Unix-socket path goes percent-encoded into the URL host slot — sqlx
# rejects libpq's `?host=/path` form with "empty host".

locals {
  cloudsql_socket_path    = "/cloudsql/${google_sql_database_instance.torch.connection_name}"
  cloudsql_socket_encoded = replace(replace(local.cloudsql_socket_path, "/", "%2F"), ":", "%3A")
}

# Writer URL — torch_ingest role. Mounted by ingest service + backfill job ONLY.
resource "google_secret_manager_secret" "ingest_db_url" {
  secret_id = "torch-ingest-db-url"
  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "ingest_db_url" {
  secret      = google_secret_manager_secret.ingest_db_url.id
  secret_data = "postgres://torch_ingest:${random_password.ingest_db.result}@${local.cloudsql_socket_encoded}/torch"
}

# Read URL — torch_api role (SELECT only). The api service NEVER sees a
# writable connection string; the projection layer is read-only by grant.
resource "google_secret_manager_secret" "api_db_url" {
  secret_id = "torch-api-db-url"
  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "api_db_url" {
  secret      = google_secret_manager_secret.api_db_url.id
  secret_data = "postgres://torch_api:${random_password.api_db.result}@${local.cloudsql_socket_encoded}/torch"
}

# Superuser password — bootstrap-schema.sh only. Never mounted into any service.
resource "google_secret_manager_secret" "pg_superuser_password" {
  secret_id = "torch-pg-superuser-password"
  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "pg_superuser_password" {
  secret      = google_secret_manager_secret.pg_superuser_password.id
  secret_data = random_password.pg_superuser.result
}

resource "google_secret_manager_secret" "laserstream_token" {
  secret_id = "torch-laserstream-token"
  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "laserstream_token" {
  secret      = google_secret_manager_secret.laserstream_token.id
  secret_data = var.laserstream_token
}

resource "google_secret_manager_secret" "rpc_url" {
  secret_id = "torch-rpc-url"
  replication {
    auto {}
  }
}

resource "google_secret_manager_secret_version" "rpc_url" {
  secret      = google_secret_manager_secret.rpc_url.id
  secret_data = var.rpc_url_helius
}
