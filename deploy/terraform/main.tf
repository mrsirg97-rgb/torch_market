# Provider + version pins. State is local — single-developer, single-project.
# Swap to a `gcs` backend if state ever needs sharing.

terraform {
  required_version = ">= 1.5"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.0"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.6"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
}

locals {
  ar_repo = "torch"

  # Cloud Run v2 validates the image at service-create time; `init` sentinel
  # points at Google's hello image so first apply succeeds, then
  # build-and-push.sh + re-apply with the git SHA rolls the real ones.
  is_placeholder    = var.image_tag == "init"
  placeholder_image = "us-docker.pkg.dev/cloudrun/container/hello"

  registry_base = "${var.region}-docker.pkg.dev/${var.project_id}/${local.ar_repo}"
  ingest_image  = local.is_placeholder ? local.placeholder_image : "${local.registry_base}/ingest:${var.image_tag}"
  api_image     = local.is_placeholder ? local.placeholder_image : "${local.registry_base}/api:${var.image_tag}"
  ui_image      = local.is_placeholder ? local.placeholder_image : "${local.registry_base}/ui:${var.image_tag}"

  api_domain = "api.${var.domain}"
}
