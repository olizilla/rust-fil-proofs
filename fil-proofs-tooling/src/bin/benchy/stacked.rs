use std::fs::{File, OpenOptions};
use std::time::Duration;
use std::{io, u32};

use anyhow::bail;
use bellperson::Circuit;
use chrono::Utc;
use log::info;
use memmap::MmapMut;
use memmap::MmapOptions;
use merkletree::store::{StoreConfig, DEFAULT_CACHED_ABOVE_BASE_LAYER};
use paired::bls12_381::Bls12;
use rand::Rng;

use fil_proofs_tooling::{measure, FuncMeasurement, Metadata};
use storage_proofs::circuit::metric::MetricCS;
use storage_proofs::circuit::stacked::StackedCompound;
use storage_proofs::compound_proof::{self, CompoundProof};
use storage_proofs::drgraph::*;
use storage_proofs::hasher::{Blake2sHasher, Domain, Hasher, PedersenHasher, Sha256Hasher};
use storage_proofs::porep::PoRep;
use storage_proofs::proof::ProofScheme;
use storage_proofs::stacked::{
    self, CacheKey, ChallengeRequirements, StackedConfig, StackedDrg, TemporaryAuxCache, EXP_DEGREE,
};
use tempfile::TempDir;

fn file_backed_mmap_from_zeroes(n: usize, use_tmp: bool) -> anyhow::Result<MmapMut> {
    let file: File = if use_tmp {
        tempfile::tempfile().unwrap()
    } else {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(format!("./stacked-data-{:?}", Utc::now()))
            .unwrap()
    };

    file.set_len(32 * n as u64).unwrap();

    let map = unsafe { MmapOptions::new().map_mut(&file) }?;

    Ok(map)
}

fn dump_proof_bytes<H: Hasher>(
    all_partition_proofs: &[stacked::Proof<H, Sha256Hasher>],
) -> anyhow::Result<()> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .open(format!("./proofs-{:?}", Utc::now()))
        .unwrap();

    serde_json::to_writer(file, all_partition_proofs)?;

    Ok(())
}

#[derive(Clone, Debug)]
struct Params {
    samples: usize,
    window_size_nodes: usize,
    data_size: usize,
    config: StackedConfig,
    partitions: usize,
    circuit: bool,
    groth: bool,
    bench: bool,
    extract: bool,
    use_tmp: bool,
    dump_proofs: bool,
    bench_only: bool,
    hasher: String,
}

impl From<Params> for Inputs {
    fn from(p: Params) -> Self {
        Inputs {
            sector_size: p.data_size,
            partitions: p.partitions,
            hasher: p.hasher.clone(),
            samples: p.samples,
            layers: p.config.layers(),
            partition_challenges: p.config.window_challenges.challenges_count_all(),
            total_challenges: p.config.window_challenges.challenges_count_all() * p.partitions,
            config: p.config,
        }
    }
}

fn generate_report<H: 'static>(params: Params, cache_dir: &TempDir) -> anyhow::Result<Report>
where
    H: Hasher,
{
    let FuncMeasurement {
        cpu_time: total_cpu_time,
        wall_time: total_wall_time,
        return_value: mut report,
    } = measure(|| {
        let mut report = Report {
            inputs: Inputs::from(params.clone()),
            outputs: Default::default(),
        };

        let Params {
            samples,
            data_size,
            config,
            partitions,
            circuit,
            groth,
            bench,
            extract,
            use_tmp,
            dump_proofs,
            bench_only,
            window_size_nodes,
            ..
        } = &params;

        // MT for original data is always named tree-d, and it will be
        // referenced later in the process as such.
        let store_config = StoreConfig::new(
            cache_dir.path(),
            CacheKey::CommDTree.to_string(),
            DEFAULT_CACHED_ABOVE_BASE_LAYER,
        );

        let mut total_proving_wall_time = Duration::new(0, 0);
        let mut total_proving_cpu_time = Duration::new(0, 0);

        let rng = &mut rand::thread_rng();
        let nodes = data_size / 32;

        let replica_id = H::Domain::random(rng);
        let sp = stacked::SetupParams {
            nodes,
            degree: BASE_DEGREE,
            expansion_degree: EXP_DEGREE,
            seed: new_seed(),
            config: config.clone(),
            window_size_nodes: *window_size_nodes,
        };

        let pp = StackedDrg::<H, Sha256Hasher>::setup(&sp)?;

        let (pub_in, priv_in, d) = if *bench_only {
            (None, None, None)
        } else {
            let mut data = file_backed_mmap_from_zeroes(nodes, *use_tmp)?;
            let seed = rng.gen();

            let FuncMeasurement {
                cpu_time: replication_cpu_time,
                wall_time: replication_wall_time,
                return_value: (pub_inputs, priv_inputs),
            } = measure(|| {
                let (tau, (p_aux, t_aux)) = StackedDrg::<H, Sha256Hasher>::replicate(
                    &pp,
                    &replica_id,
                    &mut data,
                    None,
                    Some(store_config.clone()),
                )?;

                let pb = stacked::PublicInputs::<H::Domain, <Sha256Hasher as Hasher>::Domain> {
                    replica_id,
                    seed,
                    tau: Some(tau),
                    k: Some(0),
                };

                // Convert TemporaryAux to TemporaryAuxCache, which instantiates all
                // elements based on the configs stored in TemporaryAux.
                let t_aux =
                    TemporaryAuxCache::new(&t_aux).expect("failed to restore contents of t_aux");

                let pv = stacked::PrivateInputs { p_aux, t_aux };

                Ok((pb, pv))
            })?;

            let avg_duration = |duration: Duration, data_size: &usize| {
                if *data_size > (u32::MAX as usize) {
                    // Duration only supports division by u32, so if data_size (of type usize) is larger,
                    // we have to jump through some hoops to get the value we want, which is duration / size.
                    // Consider: x = size / max
                    //           y = duration / x = duration * max / size
                    //           y / max = duration * max / size * max = duration / size
                    let x = *data_size as f64 / f64::from(u32::MAX);
                    let y = duration / x as u32;
                    y / u32::MAX
                } else {
                    duration / (*data_size as u32)
                }
            };

            report.outputs.replication_wall_time_ms =
                Some(replication_wall_time.as_millis() as u64);
            report.outputs.replication_cpu_time_ms = Some(replication_cpu_time.as_millis() as u64);

            report.outputs.replication_wall_time_ns_per_byte =
                Some(avg_duration(replication_wall_time, data_size).as_nanos() as u64);
            report.outputs.replication_cpu_time_ns_per_byte =
                Some(avg_duration(replication_cpu_time, data_size).as_nanos() as u64);

            let FuncMeasurement {
                cpu_time: vanilla_proving_cpu_time,
                wall_time: vanilla_proving_wall_time,
                return_value: all_partition_proofs,
            } = measure(|| {
                StackedDrg::<H, Sha256Hasher>::prove_all_partitions(
                    &pp,
                    &pub_inputs,
                    &priv_inputs,
                    *partitions,
                )
            })?;

            report.outputs.vanilla_proving_wall_time_us =
                Some(vanilla_proving_wall_time.as_micros() as u64);
            report.outputs.vanilla_proving_cpu_time_us =
                Some(vanilla_proving_cpu_time.as_micros() as u64);

            total_proving_wall_time += vanilla_proving_wall_time;
            total_proving_cpu_time += vanilla_proving_cpu_time;

            if *dump_proofs {
                dump_proof_bytes(&all_partition_proofs)?;
            }

            let mut total_verification_time = FuncMeasurement {
                cpu_time: Duration::new(0, 0),
                wall_time: Duration::new(0, 0),
                return_value: (),
            };

            for _ in 0..*samples {
                let m = measure(|| {
                    let verified = StackedDrg::<H, Sha256Hasher>::verify_all_partitions(
                        &pp,
                        &pub_inputs,
                        &all_partition_proofs,
                    )?;

                    if !verified {
                        panic!("verification failed");
                    }

                    Ok(())
                })?;

                total_verification_time.cpu_time += m.cpu_time;
                total_verification_time.wall_time += m.wall_time;

                report.outputs.vanilla_verification_wall_time_us =
                    Some(m.wall_time.as_micros() as u64);
                report.outputs.vanilla_verification_cpu_time_us =
                    Some(m.cpu_time.as_micros() as u64);
            }

            let avg_seconds = |duration: Duration, samples: &usize| {
                let n = duration / *samples as u32;
                f64::from(n.subsec_nanos()) / 1_000_000_000f64 + (n.as_secs() as f64)
            };

            report.outputs.verifying_wall_time_avg_ms =
                Some((avg_seconds(total_verification_time.wall_time, samples) * 1000.0) as u64);
            report.outputs.verifying_cpu_time_avg_ms =
                Some((avg_seconds(total_verification_time.cpu_time, samples) * 1000.0) as u64);

            (Some(pub_inputs), Some(priv_inputs), Some(data))
        };

        if *circuit || *groth || *bench {
            let CircuitWorkMeasurement {
                cpu_time,
                wall_time,
            } = do_circuit_work(&pp, pub_in, priv_in, &params, &mut report)?;
            total_proving_wall_time += wall_time;
            total_proving_cpu_time += cpu_time;
        }

        if let Some(data) = d {
            if *extract {
                let m = measure(|| {
                    StackedDrg::<H, Sha256Hasher>::extract_all(
                        &pp,
                        &replica_id,
                        &data,
                        Some(store_config.clone()),
                    )
                })?;

                assert_ne!(&(*data), m.return_value.as_slice());
                report.outputs.extracting_wall_time_ms = Some(m.wall_time.as_millis() as u64);
                report.outputs.extracting_cpu_time_ms = Some(m.cpu_time.as_millis() as u64);
            }
        }

        // total proving time is the sum of "the circuit work" and vanilla
        // proving time
        report.outputs.total_proving_wall_time_ms =
            Some(total_proving_wall_time.as_millis() as u64);
        report.outputs.total_proving_cpu_time_ms = Some(total_proving_cpu_time.as_millis() as u64);

        Ok(report)
    })?;

    report.outputs.total_report_wall_time_ms = total_wall_time.as_millis() as u64;
    report.outputs.total_report_cpu_time_ms = total_cpu_time.as_millis() as u64;

    Ok(report)
}

struct CircuitWorkMeasurement {
    cpu_time: Duration,
    wall_time: Duration,
}

fn do_circuit_work<H: 'static + Hasher>(
    pp: &<StackedDrg<H, Sha256Hasher> as ProofScheme>::PublicParams,
    pub_in: Option<<StackedDrg<H, Sha256Hasher> as ProofScheme>::PublicInputs>,
    priv_in: Option<<StackedDrg<H, Sha256Hasher> as ProofScheme>::PrivateInputs>,
    params: &Params,
    report: &mut Report,
) -> anyhow::Result<CircuitWorkMeasurement> {
    let mut proving_wall_time = Duration::new(0, 0);
    let mut proving_cpu_time = Duration::new(0, 0);

    let Params {
        samples,
        partitions,
        circuit,
        groth,
        bench,
        ..
    } = params;

    let compound_public_params = compound_proof::PublicParams {
        vanilla_params: pp.clone(),
        partitions: Some(*partitions),
    };

    if *bench || *circuit {
        info!("Generating blank circuit");
        let mut cs = MetricCS::<Bls12>::new();
        <StackedCompound as CompoundProof<_, StackedDrg<H, Sha256Hasher>, _>>::blank_circuit(&pp)
            .synthesize(&mut cs)?;

        report.outputs.circuit_num_inputs = Some(cs.num_inputs() as u64);
        report.outputs.circuit_num_constraints = Some(cs.num_constraints() as u64);
    }

    if *groth {
        info!("Generating Groth Proof");
        let pub_inputs = pub_in.expect("missing public inputs");
        let priv_inputs = priv_in.expect("missing private inputs");

        // TODO: The time measured for Groth proving also includes parameter loading (which can be long)
        // and vanilla proving, which may also be.
        // For now, analysis should note and subtract out these times.
        // We should implement a method of CompoundProof, which will skip vanilla proving.
        // We should also allow the serialized vanilla proofs to be passed (as a file) to the example
        // and skip replication/vanilla-proving entirely.
        let gparams =
            <StackedCompound as CompoundProof<_, StackedDrg<H, Sha256Hasher>, _>>::groth_params(
                &compound_public_params.vanilla_params,
            )?;

        let multi_proof = {
            let FuncMeasurement {
                wall_time,
                cpu_time,
                return_value,
            } = measure(|| {
                StackedCompound::prove(&compound_public_params, &pub_inputs, &priv_inputs, &gparams)
            })?;
            proving_wall_time += wall_time;
            proving_cpu_time += cpu_time;
            return_value
        };

        let verified = {
            let mut total_groth_verifying_wall_time = Duration::new(0, 0);
            let mut total_groth_verifying_cpu_time = Duration::new(0, 0);

            let mut result = true;
            for _ in 0..*samples {
                let cur_result = result;
                let m = measure(|| {
                    StackedCompound::verify(
                        &compound_public_params,
                        &pub_inputs,
                        &multi_proof,
                        &ChallengeRequirements {
                            minimum_challenges: 1,
                        },
                    )
                })?;

                // If one verification fails, result becomes permanently false.
                result = result && cur_result;
                total_groth_verifying_wall_time += m.wall_time;
                total_groth_verifying_cpu_time += m.cpu_time;
            }
            let avg_groth_verifying_wall_time = total_groth_verifying_wall_time / *samples as u32;
            let avg_groth_verifying_cpu_time = total_groth_verifying_cpu_time / *samples as u32;

            report.outputs.avg_groth_verifying_wall_time_ms =
                Some(avg_groth_verifying_wall_time.as_millis() as u64);
            report.outputs.avg_groth_verifying_cpu_time_ms =
                Some(avg_groth_verifying_cpu_time.as_millis() as u64);

            result
        };
        assert!(verified);
    }

    Ok(CircuitWorkMeasurement {
        cpu_time: proving_cpu_time,
        wall_time: proving_wall_time,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Inputs {
    sector_size: usize,
    partitions: usize,
    hasher: String,
    samples: usize,
    layers: usize,
    partition_challenges: usize,
    total_challenges: usize,
    config: StackedConfig,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "kebab-case")]
struct Outputs {
    avg_groth_verifying_cpu_time_ms: Option<u64>,
    avg_groth_verifying_wall_time_ms: Option<u64>,
    circuit_num_constraints: Option<u64>,
    circuit_num_inputs: Option<u64>,
    extracting_cpu_time_ms: Option<u64>,
    extracting_wall_time_ms: Option<u64>,
    replication_wall_time_ms: Option<u64>,
    replication_cpu_time_ms: Option<u64>,
    replication_wall_time_ns_per_byte: Option<u64>,
    replication_cpu_time_ns_per_byte: Option<u64>,
    total_report_cpu_time_ms: u64,
    total_report_wall_time_ms: u64,
    total_proving_cpu_time_ms: Option<u64>,
    total_proving_wall_time_ms: Option<u64>,
    vanilla_proving_cpu_time_us: Option<u64>,
    vanilla_proving_wall_time_us: Option<u64>,
    vanilla_verification_wall_time_us: Option<u64>,
    vanilla_verification_cpu_time_us: Option<u64>,
    verifying_wall_time_avg_ms: Option<u64>,
    verifying_cpu_time_avg_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Report {
    inputs: Inputs,
    outputs: Outputs,
}

impl Report {
    /// Print all results to stdout
    pub fn print(&self) {
        let wrapped = Metadata::wrap(&self).expect("failed to retrieve metadata");
        serde_json::to_writer(io::stdout(), &wrapped).expect("cannot write report-JSON to stdout");
    }
}

pub struct RunOpts {
    pub bench: bool,
    pub bench_only: bool,
    pub window_size_nodes: usize,
    pub window_challenges: usize,
    pub wrapper_challenges: usize,
    pub circuit: bool,
    pub dump: bool,
    pub extract: bool,
    pub groth: bool,
    pub hasher: String,
    pub layers: usize,
    pub no_bench: bool,
    pub no_tmp: bool,
    pub partitions: usize,
    pub size: usize,
}

pub fn run(opts: RunOpts) -> anyhow::Result<()> {
    let config = StackedConfig::new(opts.layers, opts.window_challenges, opts.wrapper_challenges);

    let params = Params {
        config,
        data_size: opts.size * 1024,
        partitions: opts.partitions,
        use_tmp: !opts.no_tmp,
        dump_proofs: opts.dump,
        groth: opts.groth,
        bench: !opts.no_bench && opts.bench,
        bench_only: opts.bench_only,
        circuit: opts.circuit,
        extract: opts.extract,
        hasher: opts.hasher,
        window_size_nodes: opts.window_size_nodes,
        samples: 5,
    };

    info!("Benchy Stacked: {:?}", &params);

    let cache_dir = tempfile::tempdir().unwrap();

    let report = match params.hasher.as_ref() {
        "pedersen" => generate_report::<PedersenHasher>(params, &cache_dir)?,
        "sha256" => generate_report::<Sha256Hasher>(params, &cache_dir)?,
        "blake2s" => generate_report::<Blake2sHasher>(params, &cache_dir)?,
        _ => bail!("invalid hasher: {}", params.hasher),
    };

    report.print();

    Ok(())
}
