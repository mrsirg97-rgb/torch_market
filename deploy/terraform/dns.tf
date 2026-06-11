# Terraform-managed Cloud DNS zone for torchmarket.dev. One-time manual step:
# after first apply, set GoDaddy's nameservers to `terraform output name_servers`.
# Everything else (records, cert validation, renewal) is IaC from then on.

resource "google_dns_managed_zone" "torch" {
  name        = "torchmarket-dev"
  dns_name    = "${var.domain}."
  description = "torchmarket.dev — managed by deploy/terraform"
}

resource "google_dns_record_set" "apex" {
  managed_zone = google_dns_managed_zone.torch.name
  name         = "${var.domain}."
  type         = "A"
  ttl          = 300
  rrdatas      = [google_compute_global_address.lb.address]
}

resource "google_dns_record_set" "www" {
  managed_zone = google_dns_managed_zone.torch.name
  name         = "www.${var.domain}."
  type         = "A"
  ttl          = 300
  rrdatas      = [google_compute_global_address.lb.address]
}

resource "google_dns_record_set" "api" {
  managed_zone = google_dns_managed_zone.torch.name
  name         = "${local.api_domain}."
  type         = "A"
  ttl          = 300
  rrdatas      = [google_compute_global_address.lb.address]
}
