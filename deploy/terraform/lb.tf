# Global external ALB: one static IP, google-managed certs, serverless NEGs.
#   torchmarket.dev, www → torch-ui   (www 301s to apex)
#   api.torchmarket.dev  → torch-api  (HTTP + WS)
# Chosen over Cloud Run domain mappings (apex support, no regional caveats).
# Managed certs provision automatically once DNS resolves through the zone —
# expect 15-60 min of PROVISIONING on first bring-up; no manual SSL anywhere.

resource "google_compute_global_address" "lb" {
  name = "torch-lb-ip"
}

resource "google_compute_managed_ssl_certificate" "torch" {
  name = "torch-cert"
  managed {
    domains = [var.domain, "www.${var.domain}", local.api_domain]
  }
}

# Serverless NEGs — one per Cloud Run service behind the LB.
resource "google_compute_region_network_endpoint_group" "ui" {
  name                  = "torch-ui-neg"
  network_endpoint_type = "SERVERLESS"
  region                = var.region
  cloud_run {
    service = google_cloud_run_v2_service.ui.name
  }
}

resource "google_compute_region_network_endpoint_group" "api" {
  name                  = "torch-api-neg"
  network_endpoint_type = "SERVERLESS"
  region                = var.region
  cloud_run {
    service = google_cloud_run_v2_service.api.name
  }
}

resource "google_compute_backend_service" "ui" {
  name                  = "torch-ui-backend"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  protocol              = "HTTPS"
  security_policy       = google_compute_security_policy.edge.id
  backend {
    group = google_compute_region_network_endpoint_group.ui.id
  }
}

resource "google_compute_backend_service" "api" {
  name                  = "torch-api-backend"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  protocol              = "HTTPS"
  security_policy       = google_compute_security_policy.edge.id
  # No timeout_sec: serverless NEGs reject it — Cloud Run's own request
  # timeout (3600s in run.tf) governs WS connection lifetime instead.
  backend {
    group = google_compute_region_network_endpoint_group.api.id
  }
}

resource "google_compute_url_map" "torch" {
  name            = "torch-urlmap"
  default_service = google_compute_backend_service.ui.id

  host_rule {
    hosts        = [local.api_domain]
    path_matcher = "api"
  }
  path_matcher {
    name = "api"
    # Public surface is an allowlist: REST + WS + health. Everything else
    # (notably /metrics) 404s at the edge without touching the service.
    default_url_redirect {
      host_redirect          = var.domain
      redirect_response_code = "MOVED_PERMANENTLY_DEFAULT"
      strip_query            = true
    }
    path_rule {
      paths   = ["/api/*", "/events", "/health", "/healthz", "/rpc", "/rpc-ws"]
      service = google_compute_backend_service.api.id
    }
  }

  host_rule {
    hosts        = ["www.${var.domain}"]
    path_matcher = "www-redirect"
  }
  path_matcher {
    name = "www-redirect"
    # default_service is required even when everything redirects.
    default_service = google_compute_backend_service.ui.id
    default_route_action {
      url_rewrite {
        host_rewrite = var.domain
      }
    }
  }
}

resource "google_compute_target_https_proxy" "torch" {
  name             = "torch-https-proxy"
  url_map          = google_compute_url_map.torch.id
  ssl_certificates = [google_compute_managed_ssl_certificate.torch.id]
}

resource "google_compute_global_forwarding_rule" "https" {
  name                  = "torch-https"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  target                = google_compute_target_https_proxy.torch.id
  ip_address            = google_compute_global_address.lb.address
  port_range            = "443"
}

# Port 80: redirect everything to HTTPS (the .dev TLD is HSTS-preloaded, so
# browsers never send plain HTTP anyway — this catches curl and old clients).
resource "google_compute_url_map" "http_redirect" {
  name = "torch-http-redirect"
  default_url_redirect {
    https_redirect         = true
    redirect_response_code = "MOVED_PERMANENTLY_DEFAULT"
    strip_query            = false
  }
}

resource "google_compute_target_http_proxy" "torch" {
  name    = "torch-http-proxy"
  url_map = google_compute_url_map.http_redirect.id
}

resource "google_compute_global_forwarding_rule" "http" {
  name                  = "torch-http"
  load_balancing_scheme = "EXTERNAL_MANAGED"
  target                = google_compute_target_http_proxy.torch.id
  ip_address            = google_compute_global_address.lb.address
  port_range            = "80"
}
