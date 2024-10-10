use std::time::Duration;

use governor::{clock::QuantaInstant, middleware::NoOpMiddleware};
use serde::{Deserialize, Serialize};
use tower_governor::{
    governor::GovernorConfigBuilder,
    key_extractor::{PeerIpKeyExtractor, SmartIpKeyExtractor},
    GovernorLayer,
};

/// Config for http rate limit.
#[derive(Debug, Deserialize, Default, Serialize, Clone)]
#[serde(rename_all = "lowercase")]
pub enum RateLimitConfig {
    /// Disable rate limit for http server.
    Disabled,
    /// Enable rate limit for http server based on the connection peer IP address.
    /// https://docs.rs/tower_governor/latest/tower_governor/key_extractor/struct.PeerIpKeyExtractor.html
    #[default]
    Simple,
    /// Enable rate limit for http server based on a smart logic for extracting the connection original IP address, useful for reverse proxies.
    /// https://docs.rs/tower_governor/latest/tower_governor/key_extractor/struct.SmartIpKeyExtractor.html
    Smart,
}

impl Default for &RateLimitConfig {
    fn default() -> Self {
        &RateLimitConfig::Simple
    }
}

/// Create the default rate-limiting layer.
///
/// This spawns a background thread to clean up the rate limiting cache.
pub fn create(
    rate_limit_config: &RateLimitConfig,
) -> Option<GovernorLayer<'static, PeerIpKeyExtractor, NoOpMiddleware<QuantaInstant>>> {
    let use_smart_extractor = match rate_limit_config {
        RateLimitConfig::Disabled => {
            tracing::info!("Rate limiting disabled");
            return None;
        }
        // By default apply rate limit
        RateLimitConfig::Simple => false,
        RateLimitConfig::Smart => true,
    };

    tracing::info!("Rate limiting enabled ({rate_limit_config:?})");

    // Configure rate limiting:
    // * allow bursts with up to five requests per IP address
    // * replenish one element every two seconds
    let mut governor_conf_builder = GovernorConfigBuilder::default();
    // governor_conf_builder.use_headers()
    governor_conf_builder.per_second(4);
    governor_conf_builder.burst_size(2);

    if use_smart_extractor {
        governor_conf_builder.key_extractor(SmartIpKeyExtractor);
    }

    let governor_conf = governor_conf_builder
        .finish()
        .expect("failed to build rate-limiting governor");

    // The governor layer needs a reference that outlives the layer.
    // The tower_governor crate recommends in its examples to use Box::leak here.
    // In the unreleased v0.4 of tower_governor this was changed to use an Arc instead.
    // https://github.com/benwis/tower-governor/pull/27
    let governor_conf = Box::leak(Box::new(governor_conf));

    // The governor needs a background task for garbage collection (to clear expired records)
    let gc_interval = Duration::from_secs(60);
    let governor_limiter = governor_conf.limiter().clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(gc_interval);
        tracing::debug!("rate limiting storage size: {}", governor_limiter.len());
        governor_limiter.retain_recent();
    });

    Some(GovernorLayer {
        config: &*governor_conf,
    })
}
