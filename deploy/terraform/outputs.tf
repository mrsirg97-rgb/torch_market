output "name_servers" {
  description = "Cloud DNS nameservers — set these at GoDaddy (one-time manual step)."
  value       = google_dns_managed_zone.torch.name_servers
}

output "lb_ip" {
  description = "Global static IP serving all three hostnames."
  value       = google_compute_global_address.lb.address
}

output "ui_url" {
  description = "Direct Cloud Run URL for the UI (pre-DNS smoke tests)."
  value       = google_cloud_run_v2_service.ui.uri
}

output "api_url" {
  description = "Direct Cloud Run URL for the API (pre-DNS smoke tests + loadtest gate)."
  value       = google_cloud_run_v2_service.api.uri
}

output "sql_connection_name" {
  description = "Cloud SQL connection name (cloud-sql-proxy + Unix-socket mounts)."
  value       = google_sql_database_instance.torch.connection_name
}

output "registry_url" {
  value = local.registry_base
}

output "backfill_command" {
  value = "gcloud run jobs execute ${google_cloud_run_v2_job.backfill.name} --region=${var.region} --wait"
}

output "pg_superuser_password" {
  value     = random_password.pg_superuser.result
  sensitive = true
}

output "ingest_db_password" {
  value     = random_password.ingest_db.result
  sensitive = true
}

output "api_db_password" {
  value     = random_password.api_db.result
  sensitive = true
}
