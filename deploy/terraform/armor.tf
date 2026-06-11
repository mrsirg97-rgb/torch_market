# Cloud Armor edge policy: per-IP rate limiting BEFORE traffic touches compute
# we pay for. Generous thresholds — invisible to humans + the UI's fetch
# patterns, a wall to scripts:
#   > 300 req/min from one IP  → 429s until back under
#   > 600 req/min from one IP  → banned 10 minutes
# WS connections count as ONE request each at the edge, so room subscribers
# are unaffected by request-rate math.

resource "google_compute_security_policy" "edge" {
  name        = "torch-edge-policy"
  description = "per-IP throttle + ban for torchmarket.dev surfaces"

  rule {
    action   = "rate_based_ban"
    priority = 1000
    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }
    rate_limit_options {
      conform_action = "allow"
      exceed_action  = "deny(429)"
      enforce_on_key = "IP"
      rate_limit_threshold {
        count        = 300
        interval_sec = 60
      }
      ban_threshold {
        count        = 600
        interval_sec = 60
      }
      ban_duration_sec = 600
    }
    description = "throttle at 300/min, ban at 600/min"
  }

  rule {
    action   = "allow"
    priority = 2147483647
    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }
    description = "default allow"
  }
}
