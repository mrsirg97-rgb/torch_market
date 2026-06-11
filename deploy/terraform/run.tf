# Cloud Run services (prompt-003 split + prompt-004):
#   torch-ingest — single writer. min=max=1 is CORRECTNESS, not cost.
#                  Internal ingress; ops listener serves /healthz + /metrics.
#   torch-api    — read tier. 1..N, always-on CPU (WS rooms + LISTEN conn).
#   torch-ui     — Next.js standalone, scales to zero.

# ----- Service accounts -------------------------------------------------

resource "google_service_account" "ingest" {
  account_id   = "torch-ingest-runner"
  display_name = "torch ingest Cloud Run runtime"
}

resource "google_service_account" "api" {
  account_id   = "torch-api-runner"
  display_name = "torch api Cloud Run runtime"
}

resource "google_service_account" "ui" {
  account_id   = "torch-ui-runner"
  display_name = "torch ui Cloud Run runtime"
}

# Secret access — each SA reads ONLY its own secrets.
resource "google_secret_manager_secret_iam_member" "ingest_db_url_access" {
  secret_id = google_secret_manager_secret.ingest_db_url.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.ingest.email}"
}

resource "google_secret_manager_secret_iam_member" "ingest_laserstream_access" {
  secret_id = google_secret_manager_secret.laserstream_token.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.ingest.email}"
}

resource "google_secret_manager_secret_iam_member" "ingest_rpc_access" {
  secret_id = google_secret_manager_secret.rpc_url.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.ingest.email}"
}

resource "google_secret_manager_secret_iam_member" "api_rpc_url_access" {
  secret_id = google_secret_manager_secret.rpc_url.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.api.email}"
}

resource "google_secret_manager_secret_iam_member" "api_db_url_access" {
  secret_id = google_secret_manager_secret.api_db_url.id
  role      = "roles/secretmanager.secretAccessor"
  member    = "serviceAccount:${google_service_account.api.email}"
}

resource "google_project_iam_member" "ingest_cloudsql" {
  project = var.project_id
  role    = "roles/cloudsql.client"
  member  = "serviceAccount:${google_service_account.ingest.email}"
}

resource "google_project_iam_member" "api_cloudsql" {
  project = var.project_id
  role    = "roles/cloudsql.client"
  member  = "serviceAccount:${google_service_account.api.email}"
}

# ----- Ingest (single writer) -------------------------------------------

resource "google_cloud_run_v2_service" "ingest" {
  name     = "torch-ingest"
  location = var.region

  deletion_protection = false

  # Nothing outside the project ever calls ingest — health checks are
  # platform-internal, and the ops /metrics is for monitoring agents.
  ingress = "INGRESS_TRAFFIC_INTERNAL_ONLY"

  template {
    service_account = google_service_account.ingest.email
    timeout         = "3600s"

    scaling {
      # Single writer. Two instances = two Laserstream subscriptions racing
      # on the idempotency keys: correct (ON CONFLICT) but wasteful, and it
      # would double pg_notify traffic. Never scale this.
      min_instance_count = 1
      max_instance_count = 1
    }

    volumes {
      name = "cloudsql"
      cloud_sql_instance {
        instances = [google_sql_database_instance.torch.connection_name]
      }
    }

    containers {
      image = local.ingest_image

      ports {
        container_port = 8080
      }

      resources {
        limits = {
          cpu    = "1"
          memory = "512Mi"
        }
        cpu_idle          = false # always-on: holds the gRPC stream
        startup_cpu_boost = true
      }

      env {
        name  = "API_BIND"
        value = "0.0.0.0:8080"
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

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  depends_on = [
    google_secret_manager_secret_version.ingest_db_url,
    google_secret_manager_secret_iam_member.ingest_db_url_access,
    google_secret_manager_secret_iam_member.ingest_laserstream_access,
    google_secret_manager_secret_iam_member.ingest_rpc_access,
    google_project_iam_member.ingest_cloudsql,
    google_artifact_registry_repository.torch,
  ]
}

# ----- API (read tier, scales) ------------------------------------------

resource "google_cloud_run_v2_service" "api" {
  name     = "torch-api"
  location = var.region

  deletion_protection = false

  # LB-only: the run.app URL stops serving the public; the ALB is the single
  # door, and its path rules decide the public surface (/api/*, /events,
  # /health — NOT /metrics, which stays in-project for monitoring).
  ingress = "INGRESS_TRAFFIC_INTERNAL_LOAD_BALANCER"

  template {
    service_account = google_service_account.api.email
    # WS connections live as long as the request timeout allows; clients get
    # cycled at the cap and the Resync/reconnect contract absorbs it.
    timeout = "3600s"

    # Idle WS sockets count toward request concurrency. The default (80)
    # would let ~320 sockets starve REST across 4 instances — a free DoS.
    # 500 gives sockets and queries separate room (measured 2.5k rps/instance).
    max_instance_request_concurrency = 500

    scaling {
      min_instance_count = 1
      max_instance_count = 4
    }

    volumes {
      name = "cloudsql"
      cloud_sql_instance {
        instances = [google_sql_database_instance.torch.connection_name]
      }
    }

    containers {
      image = local.api_image

      ports {
        container_port = 8081
      }

      resources {
        limits = {
          cpu    = "1"
          memory = "512Mi"
        }
        cpu_idle          = false # always-on: rooms + LISTEN between requests
        startup_cpu_boost = true
      }

      env {
        name  = "API_BIND"
        value = "0.0.0.0:8081"
      }
      env {
        name = "DATABASE_URL"
        value_source {
          secret_key_ref {
            secret  = google_secret_manager_secret.api_db_url.secret_id
            version = "latest"
          }
        }
      }
      env {
        # RPC proxy upstream (prompt-005): Helius URL with key inline. The
        # browser's RPC path now transits OUR edge — worker deprecated.
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

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  depends_on = [
    google_secret_manager_secret_version.api_db_url,
    google_secret_manager_secret_iam_member.api_db_url_access,
    google_secret_manager_secret_iam_member.api_rpc_url_access,
    google_project_iam_member.api_cloudsql,
    google_artifact_registry_repository.torch,
  ]
}

resource "google_cloud_run_v2_service_iam_member" "api_public" {
  name     = google_cloud_run_v2_service.api.name
  location = google_cloud_run_v2_service.api.location
  role     = "roles/run.invoker"
  member   = "allUsers"
}

# ----- UI ----------------------------------------------------------------

resource "google_cloud_run_v2_service" "ui" {
  name     = "torch-ui"
  location = var.region

  deletion_protection = false

  template {
    service_account = google_service_account.ui.email

    scaling {
      min_instance_count = 0
      max_instance_count = 4
    }

    containers {
      image = local.ui_image

      ports {
        container_port = 3000
      }

      resources {
        limits = {
          cpu    = "1"
          memory = "512Mi"
        }
      }
    }
  }

  traffic {
    type    = "TRAFFIC_TARGET_ALLOCATION_TYPE_LATEST"
    percent = 100
  }

  depends_on = [google_artifact_registry_repository.torch]
}

resource "google_cloud_run_v2_service_iam_member" "ui_public" {
  name     = google_cloud_run_v2_service.ui.name
  location = google_cloud_run_v2_service.ui.location
  role     = "roles/run.invoker"
  member   = "allUsers"
}
