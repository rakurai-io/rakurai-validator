//! P2C region autoconfig: fetch endpoints, probe gRPC RTT, and rank by latency.

use {
    crate::proxy::{ProxyError, endpoint_from_url, sanitize_status_message_for_influx},
    ahash::HashMapExt,
    itertools::Itertools,
    p2c_protos::pre_conf::block_engine::{
        BlockEngineEndpoint, GetBlockEngineEndpointRequest,
        block_engine_validator_client::BlockEngineValidatorClient,
    },
    std::{
        collections::hash_map::Entry,
        net::{SocketAddr, ToSocketAddrs},
        time::{Duration, Instant},
    },
    thiserror::Error,
    tokio::{task, time::timeout},
    tonic::transport::Endpoint,
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
pub struct AutoconfigMetrics {
    pub autoconfig: &'static str,
    pub ping: &'static str,
    pub error: &'static str,
}

pub const P2C_AUTOCONFIG_METRICS: AutoconfigMetrics = AutoconfigMetrics {
    autoconfig: "p2c-autoconfig",
    ping: "p2c-autoconfig_ping",
    error: "p2c-autoconfig_error",
};

#[derive(Error, Debug)]
enum ProbeError {
    #[error(transparent)]
    BuildEndpoint(#[from] ProxyError),

    #[error("gRPC connect timeout")]
    ConnectTimeout,

    #[error("gRPC connect error: {0}")]
    Connect(#[from] tonic::transport::Error),

    #[error("gRPC request error: {0}")]
    Request(#[from] tonic::Status),

    #[error("no successful probe samples")]
    NoSuccessfulSamples,
}

pub fn get_endpoint(block_engine_url: &str) -> Result<Endpoint, ProxyError> {
    endpoint_from_url(
        block_engine_url,
        || {
            ProxyError::BlockEngineEndpointError(format!(
                "invalid block engine url value: {block_engine_url}",
            ))
        },
        || {
            ProxyError::BlockEngineEndpointError(format!(
                "failed to set tls_config for block engine: {block_engine_url}",
            ))
        },
    )
}

pub async fn get_block_engine_endpoints(
    backend_endpoint: &Endpoint,
) -> crate::proxy::Result<p2c_protos::pre_conf::block_engine::GetBlockEngineEndpointResponse> {
    BlockEngineValidatorClient::connect(backend_endpoint.clone())
        .await
        .map_err(|e| ProxyError::BlockEngineConnectionError(Box::new(e)))?
        .get_block_engine_endpoints(GetBlockEngineEndpointRequest {})
        .await
        .map_err(|s| ProxyError::BlockEngineRequestError {
            code: s.code(),
            message: sanitize_status_message_for_influx(s.message()),
        })
        .map(|response| response.into_inner())
}

pub fn resolve_shredstream_receiver_address(
    address: &str,
    metrics: AutoconfigMetrics,
) -> Option<SocketAddr> {
    address
        .to_socket_addrs()
        .inspect_err(|e| {
            datapoint_warn!(
                metrics.error,
                "type" => "shredstream_resolve",
                ("address", address, String),
                ("count", 1, i64),
                ("err", e.to_string(), String),
            );
        })
        .ok()
        .and_then(|mut shredstream_sockets| shredstream_sockets.next())
}

pub fn rank_candidate_endpoints(
    candidates: ahash::HashMap<String, (Option<SocketAddr>, u64)>,
) -> impl Iterator<Item = Result<(String, Endpoint, Option<SocketAddr>, u64), ProxyError>> {
    candidates
        .into_iter()
        .sorted_unstable_by_key(|(_url, (_shredstream_socket, latency_us))| *latency_us)
        .map(
            |(block_engine_url, (maybe_shredstream_socket, latency_us))| {
                let backend_endpoint = get_endpoint(&block_engine_url)?;
                Ok((
                    block_engine_url,
                    backend_endpoint,
                    maybe_shredstream_socket,
                    latency_us,
                ))
            },
        )
}

pub fn candidates_from_probe_or_global(
    probed: ahash::HashMap<String, (Option<SocketAddr>, u64)>,
    global: Option<BlockEngineEndpoint>,
    metrics: AutoconfigMetrics,
) -> Result<ahash::HashMap<String, (Option<SocketAddr>, u64)>, ProxyError> {
    if !probed.is_empty() {
        return Ok(probed);
    }
    let Some(global) = global else {
        return Err(ProxyError::BlockEngineEndpointError(
            "Block engine configuration failed: no reachable endpoints found".to_owned(),
        ));
    };
    Ok(ahash::HashMap::from_iter([(
        global.block_engine_url,
        (
            resolve_shredstream_receiver_address(&global.shredstream_receiver_address, metrics),
            u64::MAX,
        ),
    )]))
}

async fn probe_grpc_rtt_us(
    block_engine_url: &str,
    metrics: AutoconfigMetrics,
) -> Result<u64, ProbeError> {
    const PROBE_COUNT: usize = 3;

    // Connect once and probe multiple times so we're not ranking on handshake costs.
    let endpoint = get_endpoint(block_engine_url)?;
    let channel = timeout(CONNECTION_TIMEOUT, endpoint.connect())
        .await
        .map_err(|_| ProbeError::ConnectTimeout)??;

    let mut client = BlockEngineValidatorClient::new(channel);

    let mut best_us: u64 = u64::MAX;
    let mut any_success = false;
    for sample in 0..PROBE_COUNT {
        let start = Instant::now();
        let res = timeout(
            CONNECTION_TIMEOUT,
            client.get_block_engine_endpoints(GetBlockEngineEndpointRequest {}),
        )
        .await;
        match res {
            Ok(Ok(_resp)) => {
                let elapsed_us = start.elapsed().as_micros() as u64;
                any_success = true;
                best_us = best_us.min(elapsed_us);
                datapoint_info!(
                    metrics.ping,
                    "method" => "grpc",
                    ("endpoint", block_engine_url, String),
                    ("latency_us", elapsed_us, i64),
                    ("sample", sample, i64),
                );
            }
            Ok(Err(status)) => {
                datapoint_warn!(
                    metrics.error,
                    "type" => "probe_request",
                    ("url", block_engine_url, String),
                    ("count", 1, i64),
                    ("err", status.to_string(), String),
                );
            }
            Err(_elapsed) => {
                datapoint_warn!(
                    metrics.error,
                    "type" => "probe_timeout",
                    ("url", block_engine_url, String),
                    ("count", 1, i64),
                    ("err", "timeout", String),
                );
            }
        }
    }

    if any_success {
        Ok(best_us)
    } else {
        Err(ProbeError::NoSuccessfulSamples)
    }
}

/// Probe all candidate endpoints concurrently, aggregate best RTT per endpoint.
pub async fn probe_and_rank_endpoints(
    endpoints: &[BlockEngineEndpoint],
    metrics: AutoconfigMetrics,
) -> ahash::HashMap<
    String, /* block engine url */
    (
        Option<SocketAddr>, /* shredstream receiver, fallable when DNS can't resolve */
        u64,                /* latency us */
    ),
> {
    let mut agg_endpoints: ahash::HashMap<
        String, /* block engine url */
        (
            Option<SocketAddr>, /* shredstream receiver, fallable when DNS can't resolve */
            u64,                /* latency us */
        ),
    > = ahash::HashMap::with_capacity(endpoints.len());
    let mut best_endpoint_url = String::new();
    let mut best_endpoint_rtt_us = u64::MAX;

    let tasks = endpoints
        .iter()
        .map(|endpoint| {
            let endpoint = endpoint.clone();
            task::spawn(async move {
                let rtt_res = probe_grpc_rtt_us(&endpoint.block_engine_url, metrics).await;
                (endpoint, rtt_res)
            })
        })
        .collect_vec();

    for join_res in futures::future::join_all(tasks).await {
        let (endpoint, rtt_res) = match join_res {
            Ok(v) => v,
            Err(e) => {
                datapoint_warn!(
                    metrics.error,
                    "type" => "probe_join",
                    ("count", 1, i64),
                    ("err", e.to_string(), String),
                );
                continue;
            }
        };

        let rtt_us = match rtt_res {
            Ok(v) => v,
            Err(e) => {
                datapoint_warn!(
                    metrics.error,
                    "type" => "probe",
                    ("url", endpoint.block_engine_url.as_str(), String),
                    ("count", 1, i64),
                    ("err", e.to_string(), String),
                );
                continue;
            }
        };

        if rtt_us <= best_endpoint_rtt_us {
            best_endpoint_rtt_us = rtt_us;
            best_endpoint_url = endpoint.block_engine_url.clone();
        }

        match agg_endpoints.entry(endpoint.block_engine_url.clone()) {
            Entry::Occupied(mut ent) => {
                let (_shredstream_socket, best_rtt_us) = ent.get_mut();
                if rtt_us <= *best_rtt_us {
                    *best_rtt_us = rtt_us;
                }
            }
            Entry::Vacant(entry) => {
                let maybe_shredstream_socket = resolve_shredstream_receiver_address(
                    &endpoint.shredstream_receiver_address,
                    metrics,
                );
                entry.insert((maybe_shredstream_socket, rtt_us));
            }
        };
    }

    datapoint_info!(
        metrics.autoconfig,
        ("endpoints_count", agg_endpoints.len(), i64),
        ("best_endpoint_url", best_endpoint_url.as_str(), String),
        ("best_endpoint_latency_us", best_endpoint_rtt_us, i64),
        ("count", 1, i64),
    );

    agg_endpoints
}

pub async fn autoconfig_ranked_candidates(
    seed_url: &str,
    seed_endpoint: &Endpoint,
    metrics: AutoconfigMetrics,
) -> Result<Vec<(String, Endpoint, Option<SocketAddr>, u64)>, ProxyError> {
    match get_block_engine_endpoints(seed_endpoint).await {
        Ok(endpoints) => {
            let probed = probe_and_rank_endpoints(&endpoints.regioned_endpoints, metrics).await;
            let candidates =
                candidates_from_probe_or_global(probed, endpoints.global_endpoint, metrics)?;
            rank_candidate_endpoints(candidates).collect()
        }
        Err(error) => {
            // Older / Relayer-only hosts may omit GetBlockEngineEndpoints (or still
            // lack any discovery RPC). Do not block P2C — dial the configured URL.
            datapoint_warn!(
                metrics.error,
                "type" => "discovery_fallback",
                ("url", seed_url, String),
                ("count", 1, i64),
                ("error", error.to_string(), String),
            );
            Ok(vec![(
                seed_url.to_string(),
                seed_endpoint.clone(),
                None,
                u64::MAX,
            )])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoconfig_ranking_preserves_candidate_endpoint_and_shredstream() {
        let first_url = "https://localhost:1111";
        let configured_url = "https://localhost:2222";
        let first_shredstream = "127.0.0.1:1111".parse().unwrap();
        let configured_shredstream = "127.0.0.1:2222".parse().unwrap();
        let candidates = ahash::HashMap::from_iter([
            (first_url.to_string(), (Some(first_shredstream), 1)),
            (
                configured_url.to_string(),
                (Some(configured_shredstream), 2),
            ),
        ]);

        let ranked = rank_candidate_endpoints(candidates)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let attempts = ranked
            .iter()
            .map(|(url, endpoint, shredstream, _latency_us)| {
                (
                    url.as_str(),
                    endpoint.uri().authority().unwrap().as_str(),
                    *shredstream,
                )
            })
            .collect_vec();

        assert_eq!(
            attempts,
            [
                (first_url, "localhost:1111", Some(first_shredstream)),
                (
                    configured_url,
                    "localhost:2222",
                    Some(configured_shredstream),
                ),
            ]
        );
    }

    #[test]
    fn empty_probe_falls_back_to_global_endpoint() {
        let global = BlockEngineEndpoint {
            block_engine_url: "https://localhost:443".to_string(),
            shredstream_receiver_address: "127.0.0.1:9999".to_string(),
        };
        let candidates = candidates_from_probe_or_global(
            ahash::HashMap::new(),
            Some(global),
            P2C_AUTOCONFIG_METRICS,
        )
        .unwrap();
        assert_eq!(candidates.len(), 1);
        let (socket, latency) = &candidates["https://localhost:443"];
        assert_eq!(*latency, u64::MAX);
        assert_eq!(*socket, Some("127.0.0.1:9999".parse().unwrap()));
    }

    #[test]
    fn empty_probe_without_global_is_error() {
        let err = candidates_from_probe_or_global(
            ahash::HashMap::new(),
            None,
            P2C_AUTOCONFIG_METRICS,
        )
        .unwrap_err();
        assert!(matches!(err, ProxyError::BlockEngineEndpointError(_)));
    }

    #[test]
    fn probed_candidates_skip_global_fallback() {
        let probed =
            ahash::HashMap::from_iter([("https://localhost:1111".to_string(), (None, 10u64))]);
        let global = BlockEngineEndpoint {
            block_engine_url: "https://localhost:443".to_string(),
            shredstream_receiver_address: String::new(),
        };
        let candidates =
            candidates_from_probe_or_global(probed, Some(global), P2C_AUTOCONFIG_METRICS).unwrap();
        assert_eq!(candidates.len(), 1);
        assert!(candidates.contains_key("https://localhost:1111"));
    }
}
