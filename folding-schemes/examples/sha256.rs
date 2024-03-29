#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(clippy::upper_case_acronyms)]

use ark_crypto_primitives::crh::{
    sha256::{
        constraints::{Sha256Gadget, UnitVar},
        Sha256,
    },
    CRHScheme, CRHSchemeGadget,
};
use ark_ff::{BigInteger, PrimeField, ToConstraintField};
use ark_r1cs_std::{fields::fp::FpVar, ToBytesGadget, ToConstraintFieldGadget};
use ark_relations::r1cs::{ConstraintSystemRef, SynthesisError};
use core::marker::PhantomData;
use std::time::Instant;

use ark_pallas::{constraints::GVar, Fr, Projective};
use ark_vesta::{constraints::GVar as GVar2, Projective as Projective2};

use folding_schemes::commitment::pedersen::Pedersen;
use folding_schemes::folding::nova::Nova;
use folding_schemes::frontend::FCircuit;
use folding_schemes::{Error, FoldingScheme};
mod utils;
use utils::test_nova_setup;

/// This is the circuit that we want to fold, it implements the FCircuit trait.
/// The parameter z_i denotes the current state, and z_{i+1} denotes the next state which we get by
/// applying the step.
/// In this example we set z_i and z_{i+1} to be a single value, but the trait is made to support
/// arrays, so our state could be an array with different values.
#[derive(Clone, Copy, Debug)]
pub struct Sha256FCircuit<F: PrimeField> {
    _f: PhantomData<F>,
}
impl<F: PrimeField> FCircuit<F> for Sha256FCircuit<F> {
    type Params = ();

    fn new(_params: Self::Params) -> Self {
        Self { _f: PhantomData }
    }
    fn state_len(&self) -> usize {
        1
    }

    /// computes the next state values in place, assigning z_{i+1} into z_i, and computing the new
    /// z_{i+1}
    fn step_native(&self, _i: usize, z_i: Vec<F>) -> Result<Vec<F>, Error> {
        let out_bytes = Sha256::evaluate(&(), z_i[0].into_bigint().to_bytes_le()).unwrap();
        let out: Vec<F> = out_bytes.to_field_elements().unwrap();

        Ok(vec![out[0]])
    }

    /// generates the constraints for the step of F for the given z_i
    fn generate_step_constraints(
        &self,
        _cs: ConstraintSystemRef<F>,
        _i: usize,
        z_i: Vec<FpVar<F>>,
    ) -> Result<Vec<FpVar<F>>, SynthesisError> {
        let unit_var = UnitVar::default();
        let out_bytes = Sha256Gadget::evaluate(&unit_var, &z_i[0].to_bytes()?)?;
        let out = out_bytes.0.to_constraint_field()?;
        Ok(vec![out[0].clone()])
    }
}

/// cargo test --example sha256
#[cfg(test)]
pub mod tests {
    use super::*;
    use ark_r1cs_std::{alloc::AllocVar, R1CSVar};
    use ark_relations::r1cs::ConstraintSystem;

    // test to check that the Sha256FCircuit computes the same values inside and outside the circuit
    #[test]
    fn test_f_circuit() {
        let cs = ConstraintSystem::<Fr>::new_ref();

        let circuit = Sha256FCircuit::<Fr>::new(());
        let z_i = vec![Fr::from(1_u32)];

        let z_i1 = circuit.step_native(0, z_i.clone()).unwrap();

        let z_iVar = Vec::<FpVar<Fr>>::new_witness(cs.clone(), || Ok(z_i)).unwrap();
        let computed_z_i1Var = circuit
            .generate_step_constraints(cs.clone(), 0, z_iVar.clone())
            .unwrap();
        assert_eq!(computed_z_i1Var.value().unwrap(), z_i1);
    }
}

/// cargo run --release --example sha256
fn main() {
    let num_steps = 10;
    let initial_state = vec![Fr::from(1_u32)];

    let F_circuit = Sha256FCircuit::<Fr>::new(());

    println!("Prepare Nova ProverParams & VerifierParams");
    let (prover_params, verifier_params) = test_nova_setup::<Sha256FCircuit<Fr>>(F_circuit);

    /// The idea here is that eventually we could replace the next line chunk that defines the
    /// `type NOVA = Nova<...>` by using another folding scheme that fulfills the `FoldingScheme`
    /// trait, and the rest of our code would be working without needing to be updated.
    type NOVA = Nova<
        Projective,
        GVar,
        Projective2,
        GVar2,
        Sha256FCircuit<Fr>,
        Pedersen<Projective>,
        Pedersen<Projective2>,
    >;

    println!("Initialize FoldingScheme");
    let mut folding_scheme = NOVA::init(&prover_params, F_circuit, initial_state.clone()).unwrap();

    // compute a step of the IVC
    for i in 0..num_steps {
        let start = Instant::now();
        folding_scheme.prove_step().unwrap();
        println!("Nova::prove_step {}: {:?}", i, start.elapsed());
    }

    let (running_instance, incoming_instance, cyclefold_instance) = folding_scheme.instances();

    println!("Run the Nova's IVC verifier");
    NOVA::verify(
        verifier_params,
        initial_state,
        folding_scheme.state(), // latest state
        Fr::from(num_steps as u32),
        running_instance,
        incoming_instance,
        cyclefold_instance,
    )
    .unwrap();
}
