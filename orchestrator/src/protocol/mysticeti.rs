// Copyright (c) Mysten Labs, Inc.
// SPDX-License-Identifier: Apache-2.0

use std::{
    fmt::{Debug, Display},
    path::PathBuf,
    str::FromStr,
};

use mysticeti_core::{
    committee::Committee,
    config::{Parameters, PrivateConfig},
    types::AuthorityIndex,
};
use serde::{Deserialize, Serialize};

use crate::{
    benchmark::{BenchmarkParameters, BenchmarkType},
    client::Instance,
    settings::Settings,
};

use super::{ProtocolCommands, ProtocolMetrics};

/// The type of benchmarks supported by Mysticeti.
#[derive(Serialize, Deserialize, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MysticetiBenchmarkType {
    /// Percentage of shared vs owned objects; 0 means only owned objects and 100 means
    /// only shared objects.
    shared_objects_ratio: u16,
}

impl Debug for MysticetiBenchmarkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.shared_objects_ratio)
    }
}

impl Display for MysticetiBenchmarkType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}% shared objects", self.shared_objects_ratio)
    }
}

impl FromStr for MysticetiBenchmarkType {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            shared_objects_ratio: s.parse::<u16>()?.min(100),
        })
    }
}

impl BenchmarkType for MysticetiBenchmarkType {}

/// All configurations information to run a Mysticeti client or validator.
pub struct MysticetiProtocol {
    working_dir: PathBuf,
}

impl ProtocolCommands<MysticetiBenchmarkType> for MysticetiProtocol {
    fn protocol_dependencies(&self) -> Vec<&'static str> {
        vec![]
    }

    fn db_directories(&self) -> Vec<PathBuf> {
        // TODO
        vec![]
    }

    fn genesis_command<'a, I>(&self, instances: I) -> String
    where
        I: Iterator<Item = &'a Instance>,
    {
        let ips = instances
            .map(|x| x.main_ip.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let working_directory = self.working_dir.display();

        let genesis = [
            "cargo run --release --bin mysticeti --",
            "benchmark-genesis",
            &format!("--ips {ips} --working_directory {working_directory}"),
        ]
        .join(" ");

        ["source $HOME/.cargo/env", &genesis].join(" && ")
    }

    fn node_command<I>(
        &self,
        instances: I,
        _parameters: &BenchmarkParameters<MysticetiBenchmarkType>,
    ) -> Vec<(Instance, String)>
    where
        I: IntoIterator<Item = Instance>,
    {
        instances
            .into_iter()
            .enumerate()
            .map(|(i, instance)| {
                let authority = i as AuthorityIndex;
                let committee_path: PathBuf =
                    [&self.working_dir, &Committee::DEFAULT_FILENAME.into()]
                        .iter()
                        .collect();
                let parameters_path: PathBuf =
                    [&self.working_dir, &Parameters::DEFAULT_FILENAME.into()]
                        .iter()
                        .collect();
                let private_configs_path: PathBuf = [
                    &self.working_dir,
                    &PrivateConfig::default_filename(authority),
                ]
                .iter()
                .collect();

                let run = [
                    "cargo run --release --bin mysticeti --",
                    &format!(
                        "--authority {authority} --committee-path {}",
                        committee_path.display()
                    ),
                    &format!(
                        "--parameters-path {} --private-config-path {}",
                        parameters_path.display(),
                        private_configs_path.display()
                    ),
                ]
                .join(" ");
                let command = ["source $HOME/.cargo/env", &run].join(" && ");

                (instance, command)
            })
            .collect()
    }

    fn client_command<I>(
        &self,
        _instances: I,
        _parameters: &BenchmarkParameters<MysticetiBenchmarkType>,
    ) -> Vec<(Instance, String)>
    where
        I: IntoIterator<Item = Instance>,
    {
        // TODO
        vec![]
    }
}

impl MysticetiProtocol {
    /// Make a new instance of the Mysticeti protocol commands generator.
    pub fn new(settings: &Settings) -> Self {
        Self {
            working_dir: settings.working_dir.clone(),
        }
    }
}

impl ProtocolMetrics for MysticetiProtocol {
    const NODE_METRICS_PORT: u16 = 9091;
    const CLIENT_METRICS_PORT: u16 = 8081;

    const BENCHMARK_DURATION: &'static str = "benchmark_duration";
    const TOTAL_TRANSACTIONS: &'static str = "latency_s_count";
    const LATENCY_BUCKETS: &'static str = "latency_s";
    const LATENCY_SUM: &'static str = "latency_s_sum";
    const LATENCY_SQUARED_SUM: &'static str = "latency_squared_s";
}
