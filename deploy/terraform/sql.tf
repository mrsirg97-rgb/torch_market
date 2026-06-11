# Cloud SQL Postgres 16. db-g1-small (~$35/mo) — the read API's load profile
# (docs/load-test.md) is indexed single-table queries; small is plenty for
# the preview, and the tier is a tfvars-free resize later.

resource "google_sql_database_instance" "torch" {
  name             = "torch-pg"
  database_version = "POSTGRES_16"
  region           = var.region

  settings {
    edition           = "ENTERPRISE" # required for shared-CPU tiers
    tier              = "db-g1-small"
    availability_type = "ZONAL"
    disk_size         = 10
    disk_type         = "PD_SSD"

    backup_configuration {
      enabled    = true
      start_time = "03:00"
    }

    ip_configuration {
      # Public IP only so cloud-sql-proxy can reach it from a dev laptop for
      # bootstrap-schema.sh. Cloud Run uses the Unix-socket connector. The
      # proxy authenticates via IAM — no authorized_networks.
      ipv4_enabled = true
    }
  }

  # This environment we care about (prompt-004 decision 7).
  deletion_protection = true
}

resource "random_password" "pg_superuser" {
  length  = 32
  special = false
}

resource "google_sql_user" "postgres" {
  name     = "postgres"
  instance = google_sql_database_instance.torch.name
  password = random_password.pg_superuser.result
}

resource "google_sql_database" "torch" {
  name     = "torch"
  instance = google_sql_database_instance.torch.name
}

# THREE runtime roles (prompt-003/004 red line). bootstrap-schema.sh creates
# the actual Postgres roles with these passwords and the least-privilege
# grants — torch_ingest INSERT/UPDATE, torch_api SELECT only. google_sql_user
# would create cloudsqlsuperuser-equivalents, so we deliberately skip it.
resource "random_password" "ingest_db" {
  length  = 32
  special = false
}

resource "random_password" "api_db" {
  length  = 32
  special = false
}
