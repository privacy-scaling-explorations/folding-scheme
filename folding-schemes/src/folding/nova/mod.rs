/// Implements the scheme described in [Nova](https://eprint.iacr.org/2021/370.pdf) and
/// [CycleFold](https://eprint.iacr.org/2023/1192.pdf).
use ark_crypto_primitives::{
    crh::{poseidon::CRH, CRHScheme},
    sponge::{poseidon::PoseidonConfig, Absorb},
};
use ark_ec::{AffineRepr, CurveGroup, Group};
use ark_ff::{BigInteger, PrimeField};
use ark_r1cs_std::{groups::GroupOpsBounds, prelude::CurveVar};
use ark_std::fmt::Debug;
use ark_std::{One, Zero};
use core::marker::PhantomData;

use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystem};

use crate::ccs::r1cs::{extract_r1cs, extract_w_x, R1CS};
use crate::commitment::CommitmentProver;
use crate::folding::circuits::nonnative::point_to_nonnative_limbs;
use crate::frontend::FCircuit;
use crate::utils::vec::is_zero_vec;
use crate::Error;
use crate::FoldingScheme;

pub mod circuits;
pub mod cyclefold;
pub mod decider_eth;
pub mod decider_eth_circuit;
pub mod nifs;
pub mod traits;

use circuits::{AugmentedFCircuit, ChallengeGadget, CF2};
use cyclefold::{CycleFoldChallengeGadget, CycleFoldCircuit};
use nifs::NIFS;
use traits::NovaR1CS;

#[cfg(test)]
use cyclefold::CF_IO_LEN;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommittedInstance<C: CurveGroup> {
    pub cmE: C,
    pub u: C::ScalarField,
    pub cmW: C,
    pub x: Vec<C::ScalarField>,
}

impl<C: CurveGroup> CommittedInstance<C> {
    pub fn dummy(io_len: usize) -> Self {
        Self {
            cmE: C::zero(),
            u: C::ScalarField::zero(),
            cmW: C::zero(),
            x: vec![C::ScalarField::zero(); io_len],
        }
    }
}

impl<C: CurveGroup> CommittedInstance<C>
where
    <C as Group>::ScalarField: Absorb,
    <C as ark_ec::CurveGroup>::BaseField: ark_ff::PrimeField,
{
    /// hash implements the committed instance hash compatible with the gadget implemented in
    /// nova/circuits.rs::CommittedInstanceVar.hash.
    /// Returns `H(i, z_0, z_i, U_i)`, where `i` can be `i` but also `i+1`, and `U_i` is the
    /// `CommittedInstance`.
    pub fn hash(
        &self,
        poseidon_config: &PoseidonConfig<C::ScalarField>,
        i: C::ScalarField,
        z_0: Vec<C::ScalarField>,
        z_i: Vec<C::ScalarField>,
    ) -> Result<C::ScalarField, Error> {
        let (cmE_x, cmE_y) = point_to_nonnative_limbs::<C>(self.cmE)?;
        let (cmW_x, cmW_y) = point_to_nonnative_limbs::<C>(self.cmW)?;

        CRH::<C::ScalarField>::evaluate(
            poseidon_config,
            vec![
                vec![i],
                z_0,
                z_i,
                vec![self.u],
                self.x.clone(),
                cmE_x,
                cmE_y,
                cmW_x,
                cmW_y,
            ]
            .concat(),
        )
        .map_err(|e| Error::Other(e.to_string()))
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Witness<C: CurveGroup> {
    pub E: Vec<C::ScalarField>,
    pub rE: C::ScalarField,
    pub W: Vec<C::ScalarField>,
    pub rW: C::ScalarField,
}

impl<C: CurveGroup> Witness<C>
where
    <C as Group>::ScalarField: Absorb,
{
    pub fn new(w: Vec<C::ScalarField>, e_len: usize) -> Self {
        // note: at the current version, we don't use the blinding factors and we set them to 0
        // always.
        Self {
            E: vec![C::ScalarField::zero(); e_len],
            rE: C::ScalarField::zero(),
            W: w,
            rW: C::ScalarField::zero(),
        }
    }
    pub fn commit<CP: CommitmentProver<C>>(
        &self,
        params: &CP::Params,
        x: Vec<C::ScalarField>,
    ) -> Result<CommittedInstance<C>, Error> {
        let mut cmE = C::zero();
        if !is_zero_vec::<C::ScalarField>(&self.E) {
            cmE = CP::commit(params, &self.E, &self.rE)?;
        }
        let cmW = CP::commit(params, &self.W, &self.rW)?;
        Ok(CommittedInstance {
            cmE,
            u: C::ScalarField::one(),
            cmW,
            x,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ProverParams<C1, C2, CP1, CP2>
where
    C1: CurveGroup,
    C2: CurveGroup,
    CP1: CommitmentProver<C1>,
    CP2: CommitmentProver<C2>,
{
    pub poseidon_config: PoseidonConfig<C1::ScalarField>,
    pub cm_params: CP1::Params,
    pub cf_cm_params: CP2::Params,
}

#[derive(Debug, Clone)]
pub struct VerifierParams<C1: CurveGroup, C2: CurveGroup> {
    pub poseidon_config: PoseidonConfig<C1::ScalarField>,
    pub r1cs: R1CS<C1::ScalarField>,
    pub cf_r1cs: R1CS<C2::ScalarField>,
}

/// Implements Nova+CycleFold's IVC, described in [Nova](https://eprint.iacr.org/2021/370.pdf) and
/// [CycleFold](https://eprint.iacr.org/2023/1192.pdf), following the FoldingScheme trait
#[derive(Clone, Debug)]
pub struct Nova<C1, GC1, C2, GC2, FC, CP1, CP2>
where
    C1: CurveGroup,
    GC1: CurveVar<C1, CF2<C1>>,
    C2: CurveGroup,
    GC2: CurveVar<C2, CF2<C2>>,
    FC: FCircuit<C1::ScalarField>,
    CP1: CommitmentProver<C1>,
    CP2: CommitmentProver<C2>,
{
    _gc1: PhantomData<GC1>,
    _c2: PhantomData<C2>,
    _gc2: PhantomData<GC2>,
    /// R1CS of the Augmented Function circuit
    pub r1cs: R1CS<C1::ScalarField>,
    /// R1CS of the CycleFold circuit
    pub cf_r1cs: R1CS<C2::ScalarField>,
    pub poseidon_config: PoseidonConfig<C1::ScalarField>,
    /// CommitmentProver::Params over C1
    pub cm_params: CP1::Params,
    /// CycleFold CommitmentProver::Params, over C2
    pub cf_cm_params: CP2::Params,
    /// F circuit, the circuit that is being folded
    pub F: FC,
    pub i: C1::ScalarField,
    /// initial state
    pub z_0: Vec<C1::ScalarField>,
    /// current i-th state
    pub z_i: Vec<C1::ScalarField>,
    /// Nova instances
    pub w_i: Witness<C1>,
    pub u_i: CommittedInstance<C1>,
    pub W_i: Witness<C1>,
    pub U_i: CommittedInstance<C1>,

    /// CycleFold running instance
    pub cf_W_i: Witness<C2>,
    pub cf_U_i: CommittedInstance<C2>,
}

impl<C1, GC1, C2, GC2, FC, CP1, CP2> FoldingScheme<C1, C2, FC>
    for Nova<C1, GC1, C2, GC2, FC, CP1, CP2>
where
    C1: CurveGroup,
    GC1: CurveVar<C1, CF2<C1>>,
    C2: CurveGroup,
    GC2: CurveVar<C2, CF2<C2>>,
    FC: FCircuit<C1::ScalarField>,
    CP1: CommitmentProver<C1>,
    CP2: CommitmentProver<C2>,
    <C1 as CurveGroup>::BaseField: PrimeField,
    <C2 as CurveGroup>::BaseField: PrimeField,
    <C1 as Group>::ScalarField: Absorb,
    <C2 as Group>::ScalarField: Absorb,
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    for<'a> &'a GC1: GroupOpsBounds<'a, C1, GC1>,
    for<'a> &'a GC2: GroupOpsBounds<'a, C2, GC2>,
{
    type PreprocessorParam = (Self::ProverParam, FC);
    type ProverParam = ProverParams<C1, C2, CP1, CP2>;
    type VerifierParam = VerifierParams<C1, C2>;
    type CommittedInstanceWithWitness = (CommittedInstance<C1>, Witness<C1>);
    type CFCommittedInstanceWithWitness = (CommittedInstance<C2>, Witness<C2>);

    fn preprocess(
        prep_param: &Self::PreprocessorParam,
    ) -> Result<(Self::ProverParam, Self::VerifierParam), Error> {
        let (prover_params, F_circuit) = prep_param;

        let (r1cs, cf_r1cs) =
            get_r1cs::<C1, GC1, C2, GC2, FC>(&prover_params.poseidon_config, F_circuit.clone())?;

        let verifier_params = VerifierParams::<C1, C2> {
            poseidon_config: prover_params.poseidon_config.clone(),
            r1cs,
            cf_r1cs,
        };
        Ok((prover_params.clone(), verifier_params))
    }

    /// Initializes the Nova+CycleFold's IVC for the given parameters and initial state `z_0`.
    fn init(pp: &Self::ProverParam, F: FC, z_0: Vec<C1::ScalarField>) -> Result<Self, Error> {
        // prepare the circuit to obtain its R1CS
        let cs = ConstraintSystem::<C1::ScalarField>::new_ref();
        let cs2 = ConstraintSystem::<C1::BaseField>::new_ref();

        let F_clone = F.clone();
        let augmented_F_circuit =
            AugmentedFCircuit::<C1, C2, GC2, FC>::empty(&pp.poseidon_config, F);
        let cf_circuit = CycleFoldCircuit::<C1, GC1>::empty();

        augmented_F_circuit.generate_constraints(cs.clone())?;
        cs.finalize();
        let cs = cs.into_inner().ok_or(Error::NoInnerConstraintSystem)?;
        let r1cs = extract_r1cs::<C1::ScalarField>(&cs);

        cf_circuit.generate_constraints(cs2.clone())?;
        cs2.finalize();
        let cs2 = cs2.into_inner().ok_or(Error::NoInnerConstraintSystem)?;
        let cf_r1cs = extract_r1cs::<C1::BaseField>(&cs2);

        // setup the dummy instances
        let (w_dummy, u_dummy) = r1cs.dummy_instance();
        let (cf_w_dummy, cf_u_dummy) = cf_r1cs.dummy_instance();

        // W_dummy=W_0 is a 'dummy witness', all zeroes, but with the size corresponding to the
        // R1CS that we're working with.
        Ok(Self {
            _gc1: PhantomData,
            _c2: PhantomData,
            _gc2: PhantomData,
            r1cs,
            cf_r1cs,
            poseidon_config: pp.poseidon_config.clone(),
            cm_params: pp.cm_params.clone(),
            cf_cm_params: pp.cf_cm_params.clone(),
            F: F_clone,
            i: C1::ScalarField::zero(),
            z_0: z_0.clone(),
            z_i: z_0,
            w_i: w_dummy.clone(),
            u_i: u_dummy.clone(),
            W_i: w_dummy,
            U_i: u_dummy,
            // cyclefold running instance
            cf_W_i: cf_w_dummy.clone(),
            cf_U_i: cf_u_dummy.clone(),
        })
    }

    /// Implements IVC.P of Nova+CycleFold
    fn prove_step(&mut self) -> Result<(), Error> {
        let augmented_F_circuit: AugmentedFCircuit<C1, C2, GC2, FC>;
        let cf_circuit: CycleFoldCircuit<C1, GC1>;

        let z_i1 = self.F.clone().step_native(self.z_i.clone())?;

        // compute T and cmT for AugmentedFCircuit
        let (T, cmT) = self.compute_cmT()?;

        let r_bits = ChallengeGadget::<C1>::get_challenge_native(
            &self.poseidon_config,
            self.u_i.clone(),
            self.U_i.clone(),
            cmT,
        )?;
        let r_Fr = C1::ScalarField::from_bigint(BigInteger::from_bits_le(&r_bits))
            .ok_or(Error::OutOfBounds)?;

        // fold Nova instances
        let (W_i1, U_i1): (Witness<C1>, CommittedInstance<C1>) = NIFS::<C1, CP1>::fold_instances(
            r_Fr, &self.w_i, &self.u_i, &self.W_i, &self.U_i, &T, cmT,
        )?;

        // folded instance output (public input, x)
        // u_{i+1}.x = H(i+1, z_0, z_{i+1}, U_{i+1})
        let u_i1_x = U_i1.hash(
            &self.poseidon_config,
            self.i + C1::ScalarField::one(),
            self.z_0.clone(),
            z_i1.clone(),
        )?;

        if self.i == C1::ScalarField::zero() {
            // base case
            augmented_F_circuit = AugmentedFCircuit::<C1, C2, GC2, FC> {
                _gc2: PhantomData,
                poseidon_config: self.poseidon_config.clone(),
                i: Some(C1::ScalarField::zero()), // = i=0
                z_0: Some(self.z_0.clone()),      // = z_i
                z_i: Some(self.z_i.clone()),
                u_i: Some(self.u_i.clone()), // = dummy
                U_i: Some(self.U_i.clone()), // = dummy
                U_i1: Some(U_i1.clone()),
                cmT: Some(cmT),
                F: self.F.clone(),
                x: Some(u_i1_x),
                cf_u_i: None,
                cf_U_i: None,
                cf_U_i1: None,
                cf_cmT: None,
                cf_r_nonnat: None,
            };

            #[cfg(test)]
            NIFS::<C1, CP1>::verify_folded_instance(r_Fr, &self.u_i, &self.U_i, &U_i1, &cmT)?;
        } else {
            // CycleFold part:
            // get the vector used as public inputs 'x' in the CycleFold circuit
            let cf_u_i_x = [
                get_committed_instance_coordinates(&self.u_i),
                get_committed_instance_coordinates(&self.U_i),
                get_committed_instance_coordinates(&U_i1),
            ]
            .concat();

            cf_circuit = CycleFoldCircuit::<C1, GC1> {
                _gc: PhantomData,
                r_bits: Some(r_bits.clone()),
                cmT: Some(cmT),
                u_i: Some(self.u_i.clone()),
                U_i: Some(self.U_i.clone()),
                U_i1: Some(U_i1.clone()),
                x: Some(cf_u_i_x.clone()),
            };

            let cs2 = ConstraintSystem::<C1::BaseField>::new_ref();
            cf_circuit.generate_constraints(cs2.clone())?;

            let cs2 = cs2.into_inner().ok_or(Error::NoInnerConstraintSystem)?;
            let (cf_w_i, cf_x_i) = extract_w_x::<C1::BaseField>(&cs2);
            if cf_x_i != cf_u_i_x {
                return Err(Error::NotEqual);
            }

            #[cfg(test)]
            if cf_x_i.len() != CF_IO_LEN {
                return Err(Error::NotExpectedLength(cf_x_i.len(), CF_IO_LEN));
            }

            // fold cyclefold instances
            let cf_w_i = Witness::<C2>::new(cf_w_i.clone(), self.cf_r1cs.A.n_rows);
            let cf_u_i: CommittedInstance<C2> =
                cf_w_i.commit::<CP2>(&self.cf_cm_params, cf_x_i.clone())?;

            // compute T* and cmT* for CycleFoldCircuit
            let (cf_T, cf_cmT) = self.compute_cf_cmT(&cf_w_i, &cf_u_i)?;

            let cf_r_bits = CycleFoldChallengeGadget::<C2, GC2>::get_challenge_native(
                &self.poseidon_config,
                cf_u_i.clone(),
                self.cf_U_i.clone(),
                cf_cmT,
            )?;
            let cf_r_Fq = C1::BaseField::from_bigint(BigInteger::from_bits_le(&cf_r_bits))
                .ok_or(Error::OutOfBounds)?;

            let (cf_W_i1, cf_U_i1) = NIFS::<C2, CP2>::fold_instances(
                cf_r_Fq,
                &self.cf_W_i,
                &self.cf_U_i,
                &cf_w_i,
                &cf_u_i,
                &cf_T,
                cf_cmT,
            )?;

            augmented_F_circuit = AugmentedFCircuit::<C1, C2, GC2, FC> {
                _gc2: PhantomData,
                poseidon_config: self.poseidon_config.clone(),
                i: Some(self.i),
                z_0: Some(self.z_0.clone()),
                z_i: Some(self.z_i.clone()),
                u_i: Some(self.u_i.clone()),
                U_i: Some(self.U_i.clone()),
                U_i1: Some(U_i1.clone()),
                cmT: Some(cmT),
                F: self.F.clone(),
                x: Some(u_i1_x),
                // cyclefold values
                cf_u_i: Some(cf_u_i.clone()),
                cf_U_i: Some(self.cf_U_i.clone()),
                cf_U_i1: Some(cf_U_i1.clone()),
                cf_cmT: Some(cf_cmT),
                cf_r_nonnat: Some(cf_r_Fq),
            };

            self.cf_W_i = cf_W_i1.clone();
            self.cf_U_i = cf_U_i1.clone();

            #[cfg(test)]
            {
                self.cf_r1cs.check_instance_relation(&cf_w_i, &cf_u_i)?;
                self.cf_r1cs
                    .check_relaxed_instance_relation(&self.cf_W_i, &self.cf_U_i)?;
            }
        }

        let cs = ConstraintSystem::<C1::ScalarField>::new_ref();

        augmented_F_circuit.generate_constraints(cs.clone())?;

        let cs = cs.into_inner().ok_or(Error::NoInnerConstraintSystem)?;
        let (w_i1, x_i1) = extract_w_x::<C1::ScalarField>(&cs);
        if x_i1[0] != u_i1_x {
            return Err(Error::NotEqual);
        }

        #[cfg(test)]
        if x_i1.len() != 1 {
            return Err(Error::NotExpectedLength(x_i1.len(), 1));
        }

        // set values for next iteration
        self.i += C1::ScalarField::one();
        self.z_i = z_i1.clone();
        self.w_i = Witness::<C1>::new(w_i1, self.r1cs.A.n_rows);
        self.u_i = self.w_i.commit::<CP1>(&self.cm_params, vec![u_i1_x])?;
        self.W_i = W_i1.clone();
        self.U_i = U_i1.clone();

        #[cfg(test)]
        {
            self.r1cs.check_instance_relation(&self.w_i, &self.u_i)?;
            self.r1cs
                .check_relaxed_instance_relation(&self.W_i, &self.U_i)?;
        }

        Ok(())
    }

    fn state(&self) -> Vec<C1::ScalarField> {
        self.z_i.clone()
    }
    fn instances(
        &self,
    ) -> (
        Self::CommittedInstanceWithWitness,
        Self::CommittedInstanceWithWitness,
        Self::CFCommittedInstanceWithWitness,
    ) {
        (
            (self.U_i.clone(), self.W_i.clone()),
            (self.u_i.clone(), self.w_i.clone()),
            (self.cf_U_i.clone(), self.cf_W_i.clone()),
        )
    }

    /// Implements IVC.V of Nova+CycleFold
    fn verify(
        vp: Self::VerifierParam,
        z_0: Vec<C1::ScalarField>, // initial state
        z_i: Vec<C1::ScalarField>, // last state
        num_steps: C1::ScalarField,
        running_instance: Self::CommittedInstanceWithWitness,
        incomming_instance: Self::CommittedInstanceWithWitness,
        cyclefold_instance: Self::CFCommittedInstanceWithWitness,
    ) -> Result<(), Error> {
        let (U_i, W_i) = running_instance;
        let (u_i, w_i) = incomming_instance;
        let (cf_U_i, cf_W_i) = cyclefold_instance;

        if u_i.x.len() != 1 || U_i.x.len() != 1 {
            return Err(Error::IVCVerificationFail);
        }

        // check that u_i's output points to the running instance
        // u_i.X == H(i, z_0, z_i, U_i)
        let expected_u_i_x = U_i.hash(&vp.poseidon_config, num_steps, z_0, z_i.clone())?;
        if expected_u_i_x != u_i.x[0] {
            return Err(Error::IVCVerificationFail);
        }

        // check u_i.cmE==0, u_i.u==1 (=u_i is a un-relaxed instance)
        if !u_i.cmE.is_zero() || !u_i.u.is_one() {
            return Err(Error::IVCVerificationFail);
        }

        // check R1CS satisfiability
        vp.r1cs.check_instance_relation(&w_i, &u_i)?;
        // check RelaxedR1CS satisfiability
        vp.r1cs.check_relaxed_instance_relation(&W_i, &U_i)?;

        // check CycleFold RelaxedR1CS satisfiability
        vp.cf_r1cs
            .check_relaxed_instance_relation(&cf_W_i, &cf_U_i)?;

        Ok(())
    }
}

impl<C1, GC1, C2, GC2, FC, CP1, CP2> Nova<C1, GC1, C2, GC2, FC, CP1, CP2>
where
    C1: CurveGroup,
    GC1: CurveVar<C1, CF2<C1>>,
    C2: CurveGroup,
    GC2: CurveVar<C2, CF2<C2>>,
    FC: FCircuit<C1::ScalarField>,
    CP1: CommitmentProver<C1>,
    CP2: CommitmentProver<C2>,
    <C2 as CurveGroup>::BaseField: PrimeField,
    <C1 as Group>::ScalarField: Absorb,
    <C2 as Group>::ScalarField: Absorb,
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
{
    // computes T and cmT for the AugmentedFCircuit
    fn compute_cmT(&self) -> Result<(Vec<C1::ScalarField>, C1), Error> {
        NIFS::<C1, CP1>::compute_cmT(
            &self.cm_params,
            &self.r1cs,
            &self.w_i,
            &self.u_i,
            &self.W_i,
            &self.U_i,
        )
    }
    // computes T* and cmT* for the CycleFoldCircuit
    fn compute_cf_cmT(
        &self,
        cf_w_i: &Witness<C2>,
        cf_u_i: &CommittedInstance<C2>,
    ) -> Result<(Vec<C2::ScalarField>, C2), Error> {
        NIFS::<C2, CP2>::compute_cyclefold_cmT(
            &self.cf_cm_params,
            &self.cf_r1cs,
            cf_w_i,
            cf_u_i,
            &self.cf_W_i,
            &self.cf_U_i,
        )
    }
}

/// helper method to get the r1cs from the ConstraintSynthesizer
pub fn get_r1cs_from_cs<F: PrimeField>(
    circuit: impl ConstraintSynthesizer<F>,
) -> Result<R1CS<F>, Error> {
    let cs = ConstraintSystem::<F>::new_ref();
    circuit.generate_constraints(cs.clone())?;
    cs.finalize();
    let cs = cs.into_inner().ok_or(Error::NoInnerConstraintSystem)?;
    let r1cs = extract_r1cs::<F>(&cs);
    Ok(r1cs)
}

/// helper method to get the R1CS for both the AugmentedFCircuit and the CycleFold circuit
#[allow(clippy::type_complexity)]
pub fn get_r1cs<C1, GC1, C2, GC2, FC>(
    poseidon_config: &PoseidonConfig<C1::ScalarField>,
    F_circuit: FC,
) -> Result<(R1CS<C1::ScalarField>, R1CS<C2::ScalarField>), Error>
where
    C1: CurveGroup,
    GC1: CurveVar<C1, CF2<C1>>,
    C2: CurveGroup,
    GC2: CurveVar<C2, CF2<C2>>,
    FC: FCircuit<C1::ScalarField>,
    <C1 as CurveGroup>::BaseField: PrimeField,
    <C2 as CurveGroup>::BaseField: PrimeField,
    <C1 as Group>::ScalarField: Absorb,
    <C2 as Group>::ScalarField: Absorb,
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    for<'a> &'a GC1: GroupOpsBounds<'a, C1, GC1>,
    for<'a> &'a GC2: GroupOpsBounds<'a, C2, GC2>,
{
    let augmented_F_circuit =
        AugmentedFCircuit::<C1, C2, GC2, FC>::empty(poseidon_config, F_circuit);
    let cf_circuit = CycleFoldCircuit::<C1, GC1>::empty();
    let r1cs = get_r1cs_from_cs::<C1::ScalarField>(augmented_F_circuit)?;
    let cf_r1cs = get_r1cs_from_cs::<C2::ScalarField>(cf_circuit)?;
    Ok((r1cs, cf_r1cs))
}

/// helper method to get the pedersen params length for both the AugmentedFCircuit and the
/// CycleFold circuit
pub fn get_pedersen_params_len<C1, GC1, C2, GC2, FC>(
    poseidon_config: &PoseidonConfig<C1::ScalarField>,
    F_circuit: FC,
) -> Result<(usize, usize), Error>
where
    C1: CurveGroup,
    GC1: CurveVar<C1, CF2<C1>>,
    C2: CurveGroup,
    GC2: CurveVar<C2, CF2<C2>>,
    FC: FCircuit<C1::ScalarField>,
    <C1 as CurveGroup>::BaseField: PrimeField,
    <C2 as CurveGroup>::BaseField: PrimeField,
    <C1 as Group>::ScalarField: Absorb,
    <C2 as Group>::ScalarField: Absorb,
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    for<'a> &'a GC1: GroupOpsBounds<'a, C1, GC1>,
    for<'a> &'a GC2: GroupOpsBounds<'a, C2, GC2>,
{
    let (r1cs, cf_r1cs) = get_r1cs::<C1, GC1, C2, GC2, FC>(poseidon_config, F_circuit)?;
    Ok((r1cs.A.n_rows, cf_r1cs.A.n_rows))
}

pub(crate) fn get_committed_instance_coordinates<C: CurveGroup>(
    u: &CommittedInstance<C>,
) -> Vec<C::BaseField> {
    let zero = (&C::BaseField::zero(), &C::BaseField::one());

    let cmE = u.cmE.into_affine();
    let (cmE_x, cmE_y) = cmE.xy().unwrap_or(zero);

    let cmW = u.cmW.into_affine();
    let (cmW_x, cmW_y) = cmW.xy().unwrap_or(zero);
    vec![*cmE_x, *cmE_y, *cmW_x, *cmW_y]
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use ark_pallas::{constraints::GVar, Fr, Projective};
    use ark_vesta::{constraints::GVar as GVar2, Projective as Projective2};

    use crate::commitment::pedersen::Pedersen;
    use crate::frontend::tests::CubicFCircuit;
    use crate::transcript::poseidon::poseidon_test_config;

    #[test]
    fn test_ivc() {
        type NOVA = Nova<
            Projective,
            GVar,
            Projective2,
            GVar2,
            CubicFCircuit<Fr>,
            Pedersen<Projective>,
            Pedersen<Projective2>,
        >;

        let mut rng = ark_std::test_rng();
        let poseidon_config = poseidon_test_config::<Fr>();

        let F_circuit = CubicFCircuit::<Fr>::new(());
        let z_0 = vec![Fr::from(3_u32)];

        let (cm_len, cf_cm_len) =
            get_pedersen_params_len::<Projective, GVar, Projective2, GVar2, CubicFCircuit<Fr>>(
                &poseidon_config,
                F_circuit,
            )
            .unwrap();
        let pedersen_params = Pedersen::<Projective>::new_params(&mut rng, cm_len);
        let cf_pedersen_params = Pedersen::<Projective2>::new_params(&mut rng, cf_cm_len);

        let prover_params =
            ProverParams::<Projective, Projective2, Pedersen<Projective>, Pedersen<Projective2>> {
                poseidon_config: poseidon_config.clone(),
                cm_params: pedersen_params,
                cf_cm_params: cf_pedersen_params,
            };

        let mut nova = NOVA::init(&prover_params, F_circuit, z_0.clone()).unwrap();

        let num_steps: usize = 3;
        for _ in 0..num_steps {
            nova.prove_step().unwrap();
        }
        assert_eq!(Fr::from(num_steps as u32), nova.i);

        let verifier_params = VerifierParams::<Projective, Projective2> {
            poseidon_config,
            r1cs: nova.r1cs,
            cf_r1cs: nova.cf_r1cs,
        };
        NOVA::verify(
            verifier_params,
            z_0,
            nova.z_i,
            nova.i,
            (nova.U_i, nova.W_i),
            (nova.u_i, nova.w_i),
            (nova.cf_U_i, nova.cf_W_i),
        )
        .unwrap();
    }
}
