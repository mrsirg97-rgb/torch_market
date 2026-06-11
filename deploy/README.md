# GCP deploy — torchmarket.dev (prompt-004)

Terraform-managed: 3 Cloud Run services (ingest single-writer / api read tier /
ui), Cloud SQL Postgres 16, global ALB + managed certs + Cloud DNS, Secret
Manager. Adapted from the metadao-challenge template; split per prompt-003.

```
deploy/
├── terraform/
│   ├── main.tf            provider, locals, image sentinel
│   ├── variables.tf       project, region, domain, program IDs, helius
│   ├── sql.tf             Cloud SQL + 3 role passwords (superuser/ingest/api)
│   ├── secrets.tf         Secret Manager (per-role DB URLs, helius, rpc)
│   ├── registry.tf        Artifact Registry
│   ├── run.tf             3 services + SAs + least-privilege secret IAM
│   ├── backfill.tf        Cloud Run Job (one-shot history walk)
│   ├── dns.tf             Cloud DNS zone + A records
│   ├── lb.tf              global ALB, managed certs, NEGs, www→apex
│   └── outputs.tf
├── bootstrap-schema.sh    schema + torch_ingest + torch_api roles
└── build-and-push.sh      3 images, git-SHA tags
```

## Run from zero

```bash
# 0. console: create project torch-market-preview, link billing, enable APIs:
#    compute run sqladmin artifactregistry secretmanager dns

cd deploy/terraform
cp terraform.tfvars.example terraform.tfvars   # fill helius + deep_pool id
terraform init && terraform apply              # ~10 min; services on sentinel image

# 1. one-time DNS flip: set GoDaddy nameservers to
terraform output name_servers
#    managed certs flip ACTIVE 15–60 min after DNS propagates. no manual SSL.

cd ..
./bootstrap-schema.sh                          # schema + both least-privilege roles

PROJECT=torch-market-preview ./build-and-push.sh
terraform -chdir=terraform apply -var image_tag=<printed-sha>

# 2. walk history from the program's birth (fresh ID ⇒ trivially complete)
gcloud run jobs execute torch-backfill --region=us-central1 --wait

# 3. THE GATE (prompt-004 decision 8): loadtest from the public edge,
#    diff against docs/load-test.md local baseline before announcing.
cargo run --manifest-path ../loadtest/Cargo.toml --release -- \
  --url https://api.torchmarket.dev --concurrency 64 --duration 10
```

## Security model (the red line)

- `torch_ingest` (INSERT/UPDATE) — mounted by ingest service + backfill job only
- `torch_api` (SELECT only) — the api service NEVER holds a writable URL
- superuser password — bootstrap-schema.sh only, never mounted into a service
- nobody has DELETE; ingest service is INTERNAL ingress (nothing public calls it)
- prod config is Secret Manager env vars; no .env ships in any image

## Knobs that are correctness, not cost

- ingest min=max=1: single writer (chain-ordered serial ids depend on it)
- api always-on CPU: holds WS rooms + its LISTEN connection between requests
- api 1..4: each instance has its own LISTEN conn; watch Cloud SQL
  max_connections before raising max

## Cost

~$115/mo: 2 always-on vCPU (~$60) + db-g1-small (~$35) + ALB forwarding (~$18)
+ ui scale-to-zero (~free) + DNS zone (~$0.20).
