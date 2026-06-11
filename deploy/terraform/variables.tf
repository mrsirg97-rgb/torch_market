variable "project_id" {
  description = "GCP project ID (torch-market-preview). Created in console; billing linked."
  type        = string
}

variable "region" {
  description = "Region for Cloud SQL + Cloud Run + Artifact Registry."
  type        = string
  default     = "us-central1"
}

variable "domain" {
  description = "Apex domain. DNS zone is terraform-managed; flip GoDaddy nameservers to the zone's NS records after first apply."
  type        = string
  default     = "torchmarket.dev"
}

variable "torch_program_id" {
  description = "torch_market program ID (fresh world, 2026-06-10)."
  type        = string
  default     = "FghCwWojts9MbU3Pmog5peacaKrEYM5n1T68KWHy7TAh"
}

variable "deep_pool_program_id" {
  description = "deep_pool program ID (v8)."
  type        = string
  default     = "CcwF61GW14AcxCS4E2zedHXdFXy8x8GQPvfxZrs2x2eT"
}

variable "laserstream_url" {
  description = "Helius Laserstream gRPC endpoint."
  type        = string
  default     = "https://laserstream-devnet.helius-rpc.com"
}

variable "laserstream_token" {
  description = "Helius API key (Laserstream + JSON-RPC)."
  type        = string
  sensitive   = true
}

variable "rpc_url_helius" {
  description = "Helius JSON-RPC URL with key inlined (ingest resume-tip check + backfill job)."
  type        = string
  sensitive   = true
}

variable "image_tag" {
  description = "Image tag to deploy. `init` sentinel on first apply; then the git SHA from build-and-push.sh."
  type        = string
  default     = "init"
}
