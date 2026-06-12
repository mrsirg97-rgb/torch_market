# Cloud Armor edge policy: per-IP rate limiting BEFORE traffic touches compute
# we pay for. Generous thresholds — invisible to humans + the UI's fetch
# patterns, a wall to scripts:
#   > 3000 req/min from one IP → 429s until back under
#   > 6000 req/min from one IP → banned 10 minutes
# 2026-06-12 again: even 1200 was tight — the market page refetches 3-5 RPC
# calls PER account-change notification, so one active viewer during busy
# trading sustains 20+ rps. Bridge number until the fan-out is debounced /
# migrated to the rooms feed (the real fix, filed). NB shared-IP households
# and offices count as one bucket.
# Recalibrated 2026-06-12: the /rpc proxy (prompt-005) moved browser RPC
# behind this budget — an active trader watching a busy market fans out
# hundreds of req/min via account-change notifications. 300/min was a REST
# browsing budget and banned our own e2e (preflights 429'd bare → browser
# reported CORS). Still a wall to scripts; survives a hot market.
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
        count        = 3000
        interval_sec = 60
      }
      ban_threshold {
        count        = 6000
        interval_sec = 60
      }
      ban_duration_sec = 600
    }
    description = "throttle at 3000/min, ban at 6000/min"
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
