use ark_crypto_primitives::sponge::{poseidon::PoseidonConfig, Absorb};
use ark_ec::{CurveGroup, Group};
use ark_ff::{BigInteger, PrimeField};
use ark_relations::r1cs::ConstraintSynthesizer;
use ark_relations::r1cs::ConstraintSystem;
use ark_std::rand::Rng;
use ark_std::{One, Zero};
use core::marker::PhantomData;

use super::circuits::{AugmentedFCircuit, FCircuit};
use super::{nifs::NIFS, CommittedInstance, NovaR1CS, Witness};
use crate::ccs::r1cs::R1CS;
use crate::constants::N_BITS_CHALLENGE;
use crate::frontend::arkworks::{extract_r1cs, extract_z}; // TODO once Frontend trait is ready, use that
use crate::pedersen::{Params as PedersenParams, Pedersen};
use crate::transcript::Transcript;
use crate::Error;

pub struct IVC<C1, C2, FC, T>
where
    C1: CurveGroup,
    C2: CurveGroup,
    FC: FCircuit<C1::ScalarField>,
    T: Transcript<C1>,
{
    _c2: PhantomData<C2>,
    r1cs: R1CS<C1::ScalarField>,
    pub poseidon_config: PoseidonConfig<C1::ScalarField>,
    pub pedersen_params: PedersenParams<C1>,
    pub F: FC, // F circuit
    pub transcript: T,
    i: C1::ScalarField,
    z_0: Vec<C1::ScalarField>,
    z_i: Vec<C1::ScalarField>,
    w_i: Witness<C1>,
    u_i: CommittedInstance<C1>,
    W_i: Witness<C1>,
    U_i: CommittedInstance<C1>,
}

impl<C1, C2, FC, T> IVC<C1, C2, FC, T>
where
    C1: CurveGroup,
    C2: CurveGroup,
    FC: FCircuit<C1::ScalarField>,
    T: Transcript<C1>,
    <C1 as CurveGroup>::BaseField: PrimeField,
    <C1 as Group>::ScalarField: Absorb,
{
    pub fn new<R: Rng>(
        rng: &mut R,
        transcript_config: T::TranscriptConfig,
        poseidon_config: PoseidonConfig<C1::ScalarField>,
        F: FC,
        z_0: Vec<C1::ScalarField>,
    ) -> Self {
        // initialize params
        // prepare the circuit to obtain its R1CS
        let cs = ConstraintSystem::<C1::ScalarField>::new_ref();
        let augmented_F_circuit = AugmentedFCircuit::<C1, FC> {
            poseidon_config: poseidon_config.clone(),
            i: None,
            z_0: None,
            z_i: None,
            u_i: None,
            U_i: None,
            U_i1: None,
            cmT: None,
            r: None,
            F,
            x: None,
        };

        augmented_F_circuit
            .generate_constraints(cs.clone())
            .unwrap();
        cs.finalize();
        let cs = cs.into_inner().unwrap();
        let r1cs = extract_r1cs::<C1::ScalarField>(&cs);

        let transcript = T::new(&transcript_config);

        let pedersen_params = Pedersen::<C1>::new_params(rng, r1cs.A.n_rows);

        // setup the dummy instances
        let (w_dummy, u_dummy) = r1cs.dummy_instance();

        // W_i=W_0 is a 'dummy witness', all zeroes, but with the size corresponding to the R1CS that
        // we're working with.
        // Set U_i to be dummy instance
        Self {
            _c2: PhantomData,
            r1cs,
            poseidon_config,
            pedersen_params,
            F,
            transcript,
            i: C1::ScalarField::zero(),
            z_0: z_0.clone(),
            z_i: z_0,
            w_i: w_dummy.clone(),
            u_i: u_dummy.clone(),
            W_i: w_dummy.clone(),
            U_i: u_dummy.clone(),
        }
    }

    pub fn prove_step(&mut self) -> Result<(), Error> {
        let u_i1_x: C1::ScalarField;
        let augmented_F_circuit: AugmentedFCircuit<C1, FC>;
        let z_i1 = self.F.step_native(self.z_i.clone());

        let (W_i1, U_i1, cmT): (Witness<C1>, CommittedInstance<C1>, C1);

        if self.i == C1::ScalarField::zero() {
            // base case: i=0, z_i=z_0, U_i = U_d := dummy instance
            // u_1.x = H(1, z_0, z_i, U_i)
            u_i1_x = self
                .U_i
                .hash(
                    &self.poseidon_config,
                    C1::ScalarField::one(),
                    self.z_0.clone(),
                    z_i1.clone(),
                )
                .unwrap();

            (W_i1, U_i1, cmT) = (self.w_i.clone(), self.u_i.clone(), C1::generator());

            // base case
            augmented_F_circuit = AugmentedFCircuit::<C1, FC> {
                poseidon_config: self.poseidon_config.clone(),
                i: Some(C1::ScalarField::zero()), // = i=0
                z_0: Some(self.z_0.clone()),      // = z_i
                z_i: Some(self.z_i.clone()),
                u_i: Some(self.u_i.clone()), // = dummy
                U_i: Some(self.U_i.clone()), // = dummy
                U_i1: Some(U_i1.clone()),    // = dummy
                cmT: Some(cmT),
                r: Some(C1::ScalarField::one()),
                F: self.F,
                x: Some(u_i1_x),
            };
        } else {
            // TODO absorbs in transcript
            let r_bits = self.transcript.get_challenge_nbits(N_BITS_CHALLENGE);
            let r_Fr = C1::ScalarField::from_bigint(BigInteger::from_bits_le(&r_bits)).unwrap();

            self.r1cs.check_instance_relation(&self.w_i, &self.u_i)?;
            self.r1cs.check_instance_relation(&self.W_i, &self.U_i)?;

            // compute U_{i+1}
            let _T: Vec<C1::ScalarField>;
            (W_i1, U_i1, _T, cmT) = NIFS::<C1>::prove(
                &self.pedersen_params,
                r_Fr,
                &self.r1cs,
                &self.w_i,
                &self.u_i,
                &self.W_i,
                &self.U_i,
            )
            .unwrap();

            self.r1cs.check_instance_relation(&W_i1, &U_i1)?;

            // folded instance output (public input, x)
            // u_{i+1}.x = H(i+1, z_0, z_{i+1}, U_{i+1})
            u_i1_x = U_i1
                .hash(
                    &self.poseidon_config,
                    self.i + C1::ScalarField::one(),
                    self.z_0.clone(),
                    z_i1.clone(),
                )
                .unwrap();

            augmented_F_circuit = AugmentedFCircuit::<C1, FC> {
                poseidon_config: self.poseidon_config.clone(),
                i: Some(self.i),
                z_0: Some(self.z_0.clone()),
                z_i: Some(self.z_i.clone()),
                u_i: Some(self.u_i.clone()),
                U_i: Some(self.U_i.clone()),
                U_i1: Some(U_i1.clone()),
                cmT: Some(cmT),
                r: Some(r_Fr),
                F: self.F,
                x: Some(u_i1_x),
            };
        }

        let cs = ConstraintSystem::<C1::ScalarField>::new_ref();

        augmented_F_circuit
            .generate_constraints(cs.clone())
            .unwrap();

        let cs = cs.into_inner().unwrap();
        // notice that here we use 'Z' (uppercase) to denote the 'z-vector' as in the paper, not
        // the value 'z' (lowercase) which is the state
        let Z_i1 = extract_z::<C1::ScalarField>(&cs);
        let (w_i1, x_i1) = self.r1cs.split_z(&Z_i1);
        assert_eq!(x_i1.len(), 1);
        assert_eq!(x_i1[0], u_i1_x);

        // compute committed instances, w_{i+1}, u_{i+1}, which will be used as w_i, u_i, so we
        // assign them directly to w_i, u_i.
        self.w_i = Witness::<C1>::new(w_i1.clone(), self.r1cs.A.n_rows);
        self.u_i = self
            .w_i
            .commit(&self.pedersen_params, vec![u_i1_x])
            .unwrap();

        // set values for next iteration
        self.i += C1::ScalarField::one();
        self.z_i = z_i1.clone();
        self.U_i = U_i1.clone();
        self.W_i = W_i1.clone();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_pallas::{Fr, Projective};
    use ark_vesta::Projective as Projective2;

    use crate::folding::nova::circuits::tests::TestFCircuit;
    use crate::transcript::poseidon::{tests::poseidon_test_config, PoseidonTranscript};

    #[test]
    fn test_ivc() {
        let mut rng = ark_std::test_rng();
        let poseidon_config = poseidon_test_config::<Fr>();

        let F_circuit = TestFCircuit::<Fr> { _f: PhantomData };
        let z_0 = vec![Fr::from(3_u32)];

        let mut ivc =
            IVC::<Projective, Projective2, TestFCircuit<Fr>, PoseidonTranscript<Projective>>::new(
                &mut rng,
                poseidon_config.clone(), // transcript config (could be different than poseidon)
                poseidon_config,         // poseidon config
                F_circuit,
                z_0,
            );
        for _ in 0..4 {
            ivc.prove_step().unwrap();
        }

        ivc.r1cs
            .check_instance_relation(&ivc.w_i, &ivc.u_i)
            .unwrap();
        ivc.r1cs
            .check_instance_relation(&ivc.W_i, &ivc.U_i)
            .unwrap();
    }
}