#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(clippy::upper_case_acronyms)]

use ark_ec::CurveGroup;
use ark_ff::PrimeField;
use ark_std::rand::CryptoRng;
use ark_std::{fmt::Debug, rand::RngCore};
use thiserror::Error;

use crate::frontend::FCircuit;

pub mod ccs;
pub mod commitment;
pub mod constants;
pub mod folding;
pub mod frontend;
pub mod transcript;
pub mod utils;

#[derive(Debug, Error)]
pub enum Error {
    #[error("ark_relations::r1cs::SynthesisError")]
    SynthesisError(#[from] ark_relations::r1cs::SynthesisError),
    #[error("ark_serialize::SerializationError")]
    SerializationError(#[from] ark_serialize::SerializationError),
    #[error("ark_poly_commit::Error")]
    PolyCommitError(#[from] ark_poly_commit::Error),
    #[error("crate::utils::espresso::virtual_polynomial::ArithErrors")]
    ArithError(#[from] utils::espresso::virtual_polynomial::ArithErrors),
    #[error("{0}")]
    Other(String),

    #[error("Relation not satisfied")]
    NotSatisfied,
    #[error("Not equal")]
    NotEqual,
    #[error("Vectors should have the same length ({0}: {1}, {2}: {3})")]
    NotSameLength(String, usize, String, usize),
    #[error("Vector's length ({0}) is not the expected ({1})")]
    NotExpectedLength(usize, usize),
    #[error("Can not be empty")]
    Empty,
    #[error("Pedersen parameters length is not suficient (generators.len={0} < vector.len={1} unsatisfied)")]
    PedersenParamsLen(usize, usize),
    #[error("Commitment verification failed")]
    CommitmentVerificationFail,
    #[error("IVC verification failed")]
    IVCVerificationFail,
    #[error("R1CS instance is expected to not be relaxed")]
    R1CSUnrelaxedFail,
    #[error("Could not find the inner ConstraintSystem")]
    NoInnerConstraintSystem,
    #[error("Sum-check prove failed: {0}")]
    SumCheckProveError(String),
    #[error("Sum-check verify failed: {0}")]
    SumCheckVerifyError(String),
    #[error("Value out of bounds")]
    OutOfBounds,
    #[error("Could not construct the Evaluation Domain")]
    NewDomainFail,
    #[error("Feature '{0}' not supported yet")]
    NotSupportedYet(String),

    #[error(transparent)]
    ProtoGalaxy(folding::protogalaxy::ProtoGalaxyError),
}

/// FoldingScheme defines trait that is implemented by the diverse folding schemes. It is defined
/// over a cycle of curves (C1, C2), where:
/// - C1 is the main curve, which ScalarField we use as our F for al the field operations
/// - C2 is the auxiliary curve, which we use for the commitments, whose BaseField (for point
/// coordinates) are in the C1::ScalarField.
/// In other words, C1.Fq == C2.Fr, and C1.Fr == C2.Fq.
pub trait FoldingScheme<C1: CurveGroup, C2: CurveGroup, FC>: Clone + Debug
where
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    C2::BaseField: PrimeField,
    FC: FCircuit<C1::ScalarField>,
{
    type PreprocessorParam: Debug;
    type ProverParam: Debug;
    type VerifierParam: Debug;
    type CommittedInstanceWithWitness: Debug;
    type CFCommittedInstanceWithWitness: Debug; // CycleFold CommittedInstance & Witness

    fn preprocess(
        prep_param: &Self::PreprocessorParam,
    ) -> Result<(Self::ProverParam, Self::VerifierParam), Error>;

    fn init(
        pp: &Self::ProverParam,
        step_circuit: FC,
        z_0: Vec<C1::ScalarField>, // initial state
    ) -> Result<Self, Error>;

    fn prove_step(&mut self) -> Result<(), Error>;

    // returns the state at the current step
    fn state(&self) -> Vec<C1::ScalarField>;

    // returns the instances at the current step
    fn instances(
        &self,
    ) -> (
        Self::CommittedInstanceWithWitness,
        Self::CommittedInstanceWithWitness,
        Self::CFCommittedInstanceWithWitness,
    );

    fn verify(
        vp: Self::VerifierParam,
        z_0: Vec<C1::ScalarField>, // initial state
        z_i: Vec<C1::ScalarField>, // last state
        // number of steps between the initial state and the last state
        num_steps: C1::ScalarField,
        running_instance: Self::CommittedInstanceWithWitness,
        incomming_instance: Self::CommittedInstanceWithWitness,
        cyclefold_instance: Self::CFCommittedInstanceWithWitness,
    ) -> Result<(), Error>;
}

pub trait Decider<
    C1: CurveGroup,
    C2: CurveGroup,
    FC: FCircuit<C1::ScalarField>,
    FS: FoldingScheme<C1, C2, FC>,
> where
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    C2::BaseField: PrimeField,
{
    type ProverParam: Clone;
    type Proof: Clone;
    type VerifierParam;
    type PublicInput: Debug;
    type CommittedInstanceWithWitness: Debug;
    type CommittedInstance: Clone + Debug;

    fn prove(
        pp: &Self::ProverParam,
        rng: impl RngCore + CryptoRng,
        folding_scheme: FS,
    ) -> Result<Self::Proof, Error>;

    fn verify(
        vp: &Self::VerifierParam,
        i: C1::ScalarField,
        z_0: Vec<C1::ScalarField>,
        z_i: Vec<C1::ScalarField>,
        running_instance: &Self::CommittedInstance,
        proof: Self::Proof,
        // returns `Result<bool, Error>` to differentiate between an error occurred while performing
        // the verification steps, and the verification logic of the scheme not passing.
    ) -> Result<bool, Error>;
}
