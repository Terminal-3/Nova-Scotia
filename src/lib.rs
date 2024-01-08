use std::{
    collections::HashMap,
    env::current_dir,
    fs,
    path::{Path, PathBuf},
};

use crate::circom::reader::generate_witness_from_bin;
use circom::circuit::{CircomCircuit, R1CS};
use ff::Field;
use nova_snark::{
    provider::{mlkzg::Bn256EngineKZG, GrumpkinEngine},
    traits::{
        circuit::{StepCircuit, TrivialCircuit},
        snark::RelaxedR1CSSNARKTrait,
        Engine, Group,
    },
    PublicParams, RecursiveSNARK,
};
use num_bigint::BigInt;
use num_traits::Num;
use pasta_curves::pallas::Scalar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[cfg(not(target_family = "wasm"))]
use crate::circom::reader::generate_witness_from_wasm;

#[cfg(target_family = "wasm")]
use crate::circom::wasm::generate_witness_from_wasm;

pub mod circom;

pub type F<G> = <G as Engine>::Scalar;
pub type EE<G> = nova_snark::provider::ipa_pc::EvaluationEngine<G>;
pub type S<G> = nova_snark::spartan::snark::RelaxedR1CSSNARK<G, EE<G>>;
// pub type C1<G> = StepCircuit<G::Scalar>;
// pub type C2<G> = StepCircuit<E1::Scalar>;

#[derive(Clone)]
pub enum FileLocation {
    PathBuf(PathBuf),
    URL(String),
}

pub fn create_public_params<E1, E2, S1, S2>(
    r1cs: R1CS<F<E1>>,
) -> PublicParams<
    E1,
    E2,
    CircomCircuit<<E1 as Engine>::Scalar>,
    TrivialCircuit<<E2 as Engine>::Scalar>,
>
where
    E1: Engine<Base = <E2 as Engine>::Scalar>,
    E2: Engine<Base = <E1 as Engine>::Scalar>,
    S1: RelaxedR1CSSNARKTrait<E1>,
    S2: RelaxedR1CSSNARKTrait<E2>,
{
    let circuit_primary = CircomCircuit {
        r1cs,
        witness: None,
    };
    let circuit_secondary = TrivialCircuit::default();

    PublicParams::<
        E1,
        E2,
        CircomCircuit<<E1 as Engine>::Scalar>,
        TrivialCircuit<<E2 as Engine>::Scalar>,
    >::setup(
        &circuit_primary,
        &circuit_secondary,
        &*S1::ck_floor(),
        &*S2::ck_floor(),
    )
}

#[derive(Serialize, Deserialize)]
struct CircomInput {
    step_in: Vec<String>,

    #[serde(flatten)]
    extra: HashMap<String, Value>,
}

#[cfg(not(target_family = "wasm"))]
fn compute_witness<E1, E2>(
    current_public_input: Vec<String>,
    private_input: HashMap<String, Value>,
    witness_generator_file: FileLocation,
    witness_generator_output: &Path,
) -> Vec<<E1 as Engine>::Scalar>
where
    E1: Engine<Base = <E2 as Engine>::Scalar>,
    E2: Engine<Base = <E1 as Engine>::Scalar>,
{
    let decimal_stringified_input: Vec<String> = current_public_input
        .iter()
        .map(|x| BigInt::from_str_radix(x, 16).unwrap().to_str_radix(10))
        .collect();

    let input = CircomInput {
        step_in: decimal_stringified_input.clone(),
        extra: private_input.clone(),
    };

    let is_wasm = match &witness_generator_file {
        FileLocation::PathBuf(path) => path.extension().unwrap_or_default() == "wasm",
        FileLocation::URL(_) => true,
    };
    let input_json = serde_json::to_string(&input).unwrap();

    if is_wasm {
        generate_witness_from_wasm::<F<E1>>(
            &witness_generator_file,
            &input_json,
            &witness_generator_output,
        )
    } else {
        let witness_generator_file = match &witness_generator_file {
            FileLocation::PathBuf(path) => path,
            FileLocation::URL(_) => panic!("unreachable"),
        };
        generate_witness_from_bin::<F<E1>>(
            &witness_generator_file,
            &input_json,
            &witness_generator_output,
        )
    }
}

#[cfg(target_family = "wasm")]
async fn compute_witness<E1, E2>(
    current_public_input: Vec<String>,
    private_input: HashMap<String, Value>,
    witness_generator_file: FileLocation,
) -> Vec<<G1 as Group>::Scalar>
where
    E1: Engine<Base = <E2 as Engine>::Scalar>,
    E2: Engine<Base = <E1 as Engine>::Scalar>,
{
    let decimal_stringified_input: Vec<String> = current_public_input
        .iter()
        .map(|x| BigInt::from_str_radix(x, 16).unwrap().to_str_radix(10))
        .collect();

    let input = CircomInput {
        step_in: decimal_stringified_input.clone(),
        extra: private_input.clone(),
    };

    let is_wasm = match &witness_generator_file {
        FileLocation::PathBuf(path) => path.extension().unwrap_or_default() == "wasm",
        FileLocation::URL(_) => true,
    };
    let input_json = serde_json::to_string(&input).unwrap();

    if is_wasm {
        generate_witness_from_wasm::<F<E1>>(&witness_generator_file, &input_json).await
    } else {
        let root = current_dir().unwrap(); // compute path only when generating witness from a binary
        let witness_generator_output = root.join("circom_witness.wtns");
        let witness_generator_file = match &witness_generator_file {
            FileLocation::PathBuf(path) => path,
            FileLocation::URL(_) => panic!("unreachable"),
        };
        generate_witness_from_bin::<F<E1>>(
            &witness_generator_file,
            &input_json,
            &witness_generator_output,
        )
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn create_recursive_circuit<E1, E2>(
    witness_generator_file: FileLocation,
    r1cs: R1CS<F<E1>>,
    private_inputs: Vec<HashMap<String, Value>>,
    start_public_input: Vec<F<E1>>,
    pp: &PublicParams<
        E1,
        E2,
        CircomCircuit<<E1 as Engine>::Scalar>,
        TrivialCircuit<<E2 as Engine>::Scalar>,
    >,
) -> Result<
    RecursiveSNARK<
        E1,
        E2,
        CircomCircuit<<E1 as Engine>::Scalar>,
        TrivialCircuit<<E2 as Engine>::Scalar>,
    >,
    std::io::Error,
>
where
    E1: Engine<Base = <E2 as Engine>::Scalar>,
    E2: Engine<Base = <E1 as Engine>::Scalar>,
{
    let root = current_dir().unwrap();
    let witness_generator_output = root.join("circom_witness.wtns");

    let iteration_count = private_inputs.len();

    let start_public_input_hex = start_public_input
        .iter()
        .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
        .collect::<Vec<String>>();
    let mut current_public_input = start_public_input_hex.clone();

    let witness_0 = compute_witness::<E1, E2>(
        current_public_input.clone(),
        private_inputs[0].clone(),
        witness_generator_file.clone(),
        &witness_generator_output,
    );

    let circuit_0 = CircomCircuit {
        r1cs: r1cs.clone(),
        witness: Some(witness_0),
    };
    let circuit_secondary: TrivialCircuit<_> = TrivialCircuit::default();
    // let z0_secondary = vec![E2::Scalar::ZERO];

    let mut recursive_snark = RecursiveSNARK::<
        E1,
        E2,
        CircomCircuit<<E1 as Engine>::Scalar>,
        TrivialCircuit<<E2 as Engine>::Scalar>,
    >::new(
        pp,
        &circuit_0,
        &circuit_secondary,
        &start_public_input,
        &vec![E2::Scalar::ZERO],
    )
    .unwrap();

    for i in 0..iteration_count {
        let witness = compute_witness::<E1, E2>(
            current_public_input.clone(),
            private_inputs[i].clone(),
            witness_generator_file.clone(),
            &witness_generator_output,
        );

        let circuit = CircomCircuit {
            r1cs: r1cs.clone(),
            witness: Some(witness),
        };

        let current_public_output = circuit.get_public_outputs();
        current_public_input = current_public_output
            .iter()
            .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
            .collect();

        recursive_snark
            .prove_step(&pp, &circuit, &circuit_secondary)
            .unwrap();
    }
    fs::remove_file(witness_generator_output)?;

    Ok(recursive_snark)
}

#[cfg(target_family = "wasm")]
pub fn create_recursive_circuit<E1, E2>(
    witness_generator_file: FileLocation,
    r1cs: R1CS<F<E1>>,
    private_inputs: Vec<HashMap<String, Value>>,
    start_public_input: Vec<F<E1>>,
    pp: &PublicParams<
        E1,
        E2,
        CircomCircuit<<E1 as Engine>::Scalar>,
        TrivialCircuit<<E2 as Engine>::Scalar>,
    >,
) -> Result<
    RecursiveSNARK<
        E1,
        E2,
        CircomCircuit<<E1 as Engine>::Scalar>,
        TrivialCircuit<<E2 as Engine>::Scalar>,
    >,
    std::io::Error,
>
where
    E1: Engine<Base = <E2 as Engine>::Scalar>,
    E2: Engine<Base = <E1 as Engine>::Scalar>,
{
    let iteration_count = private_inputs.len();

    let start_public_input_hex = start_public_input
        .iter()
        .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
        .collect::<Vec<String>>();
    let mut current_public_input = start_public_input_hex.clone();

    let witness_0 = compute_witness::<E1, E2>(
        current_public_input.clone(),
        private_inputs[0].clone(),
        witness_generator_file.clone(),
    )
    .await;

    let circuit_0 = CircomCircuit {
        r1cs: r1cs.clone(),
        witness: Some(witness_0),
    };
    let circuit_secondary = TrivialCircuit::default();
    // let z0_secondary = vec![G2::Scalar::ZERO];

    let mut recursive_snark = RecursiveSNARK::<
        E1,
        E2,
        CircomCircuit<<E1 as Engine>::Scalar>,
        TrivialCircuit<<E2 as Engine>::Scalar>,
    >::new(
        pp,
        &circuit_0,
        &circuit_secondary,
        &start_public_input,
        &vec![E2::Scalar::ZERO],
    )
    .unwrap();

    for i in 0..iteration_count {
        let witness = compute_witness::<E1, E2>(
            current_public_input.clone(),
            private_inputs[i].clone(),
            witness_generator_file.clone(),
        )
        .await;

        let circuit = CircomCircuit {
            r1cs: r1cs.clone(),
            witness: Some(witness),
        };

        let current_public_output = circuit.get_public_outputs();
        current_public_input = current_public_output
            .iter()
            .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
            .collect();

        recursive_snark
            .prove_step(&pp, &circuit, &circuit_secondary)
            .unwrap();
    }

    Ok(recursive_snark)
}

// #[cfg(not(target_family = "wasm"))]
// pub fn continue_recursive_circuit<E1, E2, C1, C2>(
//     recursive_snark: &mut RecursiveSNARK<
//         E1,
//         E2,
//         CircomCircuit<<E1 as Engine>::Scalar>,
//         TrivialCircuit<<E2 as Engine>::Scalar>,
//     >,
//     last_zi: Vec<F<E1>>,
//     witness_generator_file: FileLocation,
//     r1cs: R1CS<F<E1>>,
//     private_inputs: Vec<HashMap<String, Value>>,
//     start_public_input: Vec<F<E1>>,
//     pp: &PublicParams<
//         E1,
//         E2,
//         CircomCircuit<<E1 as Engine>::Scalar>,
//         TrivialCircuit<<E2 as Engine>::Scalar>,
//     >,
// ) -> Result<(), std::io::Error>
// where
//     E1: Engine<Base = <E2 as Engine>::Scalar>,
//     E2: Engine<Base = <E1 as Engine>::Scalar>,
//     C1: StepCircuit<E1::Scalar>,
//     C2: StepCircuit<E2::Scalar>,
// {
//     let root = current_dir().unwrap();
//     let witness_generator_output = root.join("circom_witness.wtns");

//     let iteration_count = private_inputs.len();

//     let mut current_public_input = last_zi
//         .iter()
//         .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
//         .collect::<Vec<String>>();

//     let circuit_secondary = TrivialCircuit::default();
//     let z0_secondary = vec![E2::Scalar::ZERO];

//     for i in 0..iteration_count {
//         let witness = compute_witness::<E1, E2>(
//             current_public_input.clone(),
//             private_inputs[i].clone(),
//             witness_generator_file.clone(),
//             &witness_generator_output,
//         );

//         let circuit = CircomCircuit {
//             r1cs: r1cs.clone(),
//             witness: Some(witness),
//         };

//         let current_public_output = circuit.get_public_outputs();
//         current_public_input = current_public_output
//             .iter()
//             .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
//             .collect();

//         let res = recursive_snark.prove_step(pp, &circuit, &circuit_secondary);

//         assert!(res.is_ok());
//     }

//     fs::remove_file(witness_generator_output)?;

//     Ok(())
// }

// #[cfg(target_family = "wasm")]
// pub async fn continue_recursive_circuit<G1, G2>(
//     recursive_snark: &mut RecursiveSNARK<G1, G2, C1<G1>, C2<G2>>,
//     last_zi: Vec<F<G1>>,
//     witness_generator_file: FileLocation,
//     r1cs: R1CS<F<G1>>,
//     private_inputs: Vec<HashMap<String, Value>>,
//     start_public_input: Vec<F<G1>>,
//     pp: &PublicParams<G1, G2, C1<G1>, C2<G2>>,
// ) -> Result<(), std::io::Error>
// where
//     G1: Group<Base = <G2 as Group>::Scalar>,
//     G2: Group<Base = <G1 as Group>::Scalar>,
// {
//     let root = current_dir().unwrap();
//     let witness_generator_output = root.join("circom_witness.wtns");

//     let iteration_count = private_inputs.len();

//     let mut current_public_input = last_zi
//         .iter()
//         .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
//         .collect::<Vec<String>>();

//     let circuit_secondary = TrivialCircuit::default();
//     let z0_secondary = vec![G2::Scalar::ZERO];

//     for i in 0..iteration_count {
//         let witness = compute_witness::<G1, G2>(
//             current_public_input.clone(),
//             private_inputs[i].clone(),
//             witness_generator_file.clone(),
//         )
//         .await;

//         let circuit = CircomCircuit {
//             r1cs: r1cs.clone(),
//             witness: Some(witness),
//         };

//         let current_public_output = circuit.get_public_outputs();
//         current_public_input = current_public_output
//             .iter()
//             .map(|&x| format!("{:?}", x).strip_prefix("0x").unwrap().to_string())
//             .collect();

//         let res = recursive_snark.prove_step(
//             pp,
//             &circuit,
//             &circuit_secondary,
//             start_public_input.clone(),
//             z0_secondary.clone(),
//         );

//         assert!(res.is_ok());
//     }

//     fs::remove_file(witness_generator_output)?;

//     Ok(())
// }
