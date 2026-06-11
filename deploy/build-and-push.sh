#!/usr/bin/env bash
# Build + push all three images via CLOUD BUILD (native amd64 — local Apple
# Silicon Docker produces arm64 manifests Cloud Run rejects). Tagged with the
# git SHA; ends by printing the roll command.

set -euo pipefail
cd "$(dirname "$0")/.."

REGION="${REGION:-us-central1}"
PROJECT="${PROJECT:?set PROJECT=<gcp project id>}"
DOMAIN="${DOMAIN:-torchmarket.dev}"
NETWORK="${NETWORK:-devnet}"
REPO="${REGION}-docker.pkg.dev/${PROJECT}/torch"
TAG=$(git rev-parse --short HEAD)

gcloud builds submit \
  --project "${PROJECT}" \
  --region "${REGION}" \
  --config deploy/cloudbuild.yaml \
  --substitutions "_REPO=${REPO},_TAG=${TAG},_INDEXER_URL=https://api.${DOMAIN},_NETWORK=${NETWORK}" \
  .

echo
echo "pushed :${TAG} (amd64). roll the services:"
echo "  terraform -chdir=deploy/terraform apply -var image_tag=${TAG}"
