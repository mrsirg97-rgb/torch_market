resource "google_artifact_registry_repository" "torch" {
  location      = var.region
  repository_id = local.ar_repo
  format        = "DOCKER"
  description   = "torch images: ingest, api, ui"
}
