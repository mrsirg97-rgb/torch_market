# One-shot historical backfill (Cloud Run Job). Same image/secrets/socket as
# ingest; args=["backfill"] dispatches the subcommand. Fresh program ID means
# the walk starts at the program's birth — idempotent, safe to re-run.
#
#   gcloud run jobs execute torch-backfill --region=us-central1 --wait

resource "google_cloud_run_v2_job" "backfill" {
  name     = "torch-backfill"
  location = var.region

  deletion_protection = false

  template {
    template {
      service_account = google_service_account.ingest.email
      timeout         = "3600s"
      max_retries     = 0 # failures = config/auth problems; surface them

      volumes {
        name = "cloudsql"
        cloud_sql_instance {
          instances = [google_sql_database_instance.torch.connection_name]
        }
      }

      containers {
        image = local.ingest_image
        args  = ["backfill"]

        resources {
          limits = {
            cpu    = "1"
            memory = "512Mi"
          }
        }

        env {
          name  = "TORCH_PROGRAM_ID"
          value = var.torch_program_id
        }
        env {
          name  = "DEEP_POOL_PROGRAM_ID"
          value = var.deep_pool_program_id
        }
        env {
          name  = "LASERSTREAM_URL"
          value = var.laserstream_url
        }
        env {
          name = "DATABASE_URL"
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.ingest_db_url.secret_id
              version = "latest"
            }
          }
        }
        env {
          name = "LASERSTREAM_TOKEN"
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.laserstream_token.secret_id
              version = "latest"
            }
          }
        }
        env {
          name = "RPC_URL"
          value_source {
            secret_key_ref {
              secret  = google_secret_manager_secret.rpc_url.secret_id
              version = "latest"
            }
          }
        }

        volume_mounts {
          name       = "cloudsql"
          mount_path = "/cloudsql"
        }
      }
    }
  }

  depends_on = [
    google_secret_manager_secret_version.ingest_db_url,
    google_secret_manager_secret_iam_member.ingest_db_url_access,
  ]
}
