/// This file implements the Nova's onchain (Ethereum's EVM) decider. For non-ethereum use cases,
/// the Decider from decider.rs file will be more efficient.
/// More details can be found at the documentation page:
/// https://privacy-scaling-explorations.github.io/sonobe-docs/design/nova-decider-onchain.html
use ark_bn254::Bn254;
use ark_crypto_primitives::sponge::Absorb;
use ark_ec::{AffineRepr, CurveGroup, Group};
use ark_ff::{BigInteger, PrimeField};
use ark_groth16::Groth16;
use ark_r1cs_std::{prelude::CurveVar, ToConstraintFieldGadget};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_snark::SNARK;
use ark_std::rand::{CryptoRng, RngCore};
use ark_std::{One, Zero};
use core::marker::PhantomData;

pub use super::decider_eth_circuit::DeciderEthCircuit;
use super::decider_eth_circuit::DeciderNovaGadget;
use super::{CommittedInstance, Nova};
use crate::commitment::{
    kzg::{Proof as KZGProof, KZG},
    pedersen::Params as PedersenParams,
    CommitmentScheme,
};
use crate::folding::circuits::decider::DeciderEnabledNIFS;
use crate::folding::circuits::CF2;
use crate::folding::traits::{Inputize, WitnessOps};
use crate::frontend::FCircuit;
use crate::Error;
use crate::{Decider as DeciderTrait, FoldingScheme};

#[derive(Debug, Clone, Eq, PartialEq, CanonicalSerialize, CanonicalDeserialize)]
pub struct Proof<C, CS, S>
where
    C: CurveGroup,
    CS: CommitmentScheme<C, ProverChallenge = C::ScalarField, Challenge = C::ScalarField>,
    S: SNARK<C::ScalarField>,
{
    snark_proof: S::Proof,
    kzg_proofs: [CS::Proof; 2],
    // cmT and r are values for the last fold, U_{i+1}=NIFS.V(r, U_i, u_i, cmT), and they are
    // checked in-circuit
    cmT: C,
    r: C::ScalarField,
    // the KZG challenges are provided by the prover, but in-circuit they are checked to match
    // the in-circuit computed computed ones.
    kzg_challenges: [C::ScalarField; 2],
}

#[derive(Debug, Clone, Eq, PartialEq, CanonicalSerialize, CanonicalDeserialize)]
pub struct VerifierParam<C1, CS_VerifyingKey, S_VerifyingKey>
where
    C1: CurveGroup,
    CS_VerifyingKey: Clone + CanonicalSerialize + CanonicalDeserialize,
    S_VerifyingKey: Clone + CanonicalSerialize + CanonicalDeserialize,
{
    pub pp_hash: C1::ScalarField,
    pub snark_vp: S_VerifyingKey,
    pub cs_vp: CS_VerifyingKey,
}

/// Onchain Decider, for ethereum use cases
#[derive(Clone, Debug)]
pub struct Decider<C1, GC1, C2, GC2, FC, CS1, CS2, S, FS> {
    _c1: PhantomData<C1>,
    _gc1: PhantomData<GC1>,
    _c2: PhantomData<C2>,
    _gc2: PhantomData<GC2>,
    _fc: PhantomData<FC>,
    _cs1: PhantomData<CS1>,
    _cs2: PhantomData<CS2>,
    _s: PhantomData<S>,
    _fs: PhantomData<FS>,
}

impl<C1, GC1, C2, GC2, FC, CS1, CS2, S, FS> DeciderTrait<C1, C2, FC, FS>
    for Decider<C1, GC1, C2, GC2, FC, CS1, CS2, S, FS>
where
    C1: CurveGroup,
    GC1: CurveVar<C1, CF2<C1>> + ToConstraintFieldGadget<CF2<C1>>,
    C2: CurveGroup,
    GC2: CurveVar<C2, CF2<C2>> + ToConstraintFieldGadget<CF2<C2>>,
    FC: FCircuit<C1::ScalarField>,
    // CS1 is a KZG commitment, where challenge is C1::Fr elem
    CS1: CommitmentScheme<
        C1,
        ProverChallenge = C1::ScalarField,
        Challenge = C1::ScalarField,
        Proof = KZGProof<C1>,
    >,
    // enforce that the CS2 is Pedersen commitment scheme, since we're at Ethereum's EVM decider
    CS2: CommitmentScheme<C2, ProverParams = PedersenParams<C2>>,
    S: SNARK<C1::ScalarField>,
    FS: FoldingScheme<C1, C2, FC>,
    <C1 as CurveGroup>::BaseField: PrimeField,
    <C2 as CurveGroup>::BaseField: PrimeField,
    <C1 as Group>::ScalarField: Absorb,
    <C2 as Group>::ScalarField: Absorb,
    C1: CurveGroup<BaseField = C2::ScalarField, ScalarField = C2::BaseField>,
    // constrain FS into Nova, since this is a Decider specifically for Nova
    Nova<C1, GC1, C2, GC2, FC, CS1, CS2, false>: From<FS>,
    crate::folding::nova::ProverParams<C1, C2, CS1, CS2, false>:
        From<<FS as FoldingScheme<C1, C2, FC>>::ProverParam>,
    crate::folding::nova::VerifierParams<C1, C2, CS1, CS2, false>:
        From<<FS as FoldingScheme<C1, C2, FC>>::VerifierParam>,
{
    type PreprocessorParam = (FS::ProverParam, FS::VerifierParam);
    type ProverParam = (S::ProvingKey, CS1::ProverParams);
    type Proof = Proof<C1, CS1, S>;
    type VerifierParam = VerifierParam<C1, CS1::VerifierParams, S::VerifyingKey>;
    type PublicInput = Vec<C1::ScalarField>;
    type CommittedInstance = Vec<C1>;

    fn preprocess(
        mut rng: impl RngCore + CryptoRng,
        prep_param: Self::PreprocessorParam,
        fs: FS,
    ) -> Result<(Self::ProverParam, Self::VerifierParam), Error> {
        let circuit = DeciderEthCircuit::<C1, C2, GC2>::try_from(Nova::from(fs))?;

        // get the Groth16 specific setup for the circuit
        let (g16_pk, g16_vk) = S::circuit_specific_setup(circuit, &mut rng)
            .map_err(|e| Error::SNARKSetupFail(e.to_string()))?;

        // get the FoldingScheme prover & verifier params from Nova
        #[allow(clippy::type_complexity)]
        let nova_pp: <Nova<C1, GC1, C2, GC2, FC, CS1, CS2, false> as FoldingScheme<
            C1,
            C2,
            FC,
        >>::ProverParam = prep_param.0.clone().into();
        #[allow(clippy::type_complexity)]
        let nova_vp: <Nova<C1, GC1, C2, GC2, FC, CS1, CS2, false> as FoldingScheme<
            C1,
            C2,
            FC,
        >>::VerifierParam = prep_param.1.clone().into();
        let pp_hash = nova_vp.pp_hash()?;

        let pp = (g16_pk, nova_pp.cs_pp);
        let vp = Self::VerifierParam {
            pp_hash,
            snark_vp: g16_vk,
            cs_vp: nova_vp.cs_vp,
        };
        Ok((pp, vp))
    }

    fn prove(
        mut rng: impl RngCore + CryptoRng,
        pp: Self::ProverParam,
        folding_scheme: FS,
    ) -> Result<Self::Proof, Error> {
        let (snark_pk, cs_pk): (S::ProvingKey, CS1::ProverParams) = pp;

        let circuit = DeciderEthCircuit::<C1, C2, GC2>::try_from(Nova::from(folding_scheme))?;

        let cmT = circuit.proof;
        let r = circuit.randomness;

        // get the challenges that have been already computed when preparing the circuit inputs in
        // the above `try_from` call
        let kzg_challenges = circuit.kzg_challenges.clone();

        // generate KZG proofs
        let kzg_proofs = circuit
            .W_i1
            .get_openings()
            .iter()
            .zip(&kzg_challenges)
            .map(|((v, _), &c)| {
                CS1::prove_with_challenge(&cs_pk, c, v, &C1::ScalarField::zero(), None)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let snark_proof =
            S::prove(&snark_pk, circuit, &mut rng).map_err(|e| Error::Other(e.to_string()))?;

        Ok(Self::Proof {
            snark_proof,
            cmT,
            r,
            kzg_proofs: kzg_proofs
                .try_into()
                .map_err(|e: Vec<_>| Error::NotExpectedLength(e.len(), 2))?,
            kzg_challenges: kzg_challenges
                .try_into()
                .map_err(|e: Vec<_>| Error::NotExpectedLength(e.len(), 2))?,
        })
    }

    fn verify(
        vp: Self::VerifierParam,
        i: C1::ScalarField,
        z_0: Vec<C1::ScalarField>,
        z_i: Vec<C1::ScalarField>,
        // we don't use the instances at the verifier level, since we check them in-circuit
        running_commitments: &Self::CommittedInstance,
        incoming_commitments: &Self::CommittedInstance,
        proof: &Self::Proof,
    ) -> Result<bool, Error> {
        if i <= C1::ScalarField::one() {
            return Err(Error::NotEnoughSteps);
        }

        let Self::VerifierParam {
            pp_hash,
            snark_vp,
            cs_vp,
        } = vp;

        // 6.2. Fold the commitments
        let U_final_commitments = DeciderNovaGadget::fold_group_elements_native(
            running_commitments,
            incoming_commitments,
            Some(proof.cmT),
            proof.r,
        )?;

        let public_input = [
            &[pp_hash, i][..],
            &z_0,
            &z_i,
            &U_final_commitments
                .iter()
                .flat_map(|c| c.inputize())
                .collect::<Vec<_>>(),
            &proof.kzg_challenges,
            &proof.kzg_proofs.iter().map(|p| p.eval).collect::<Vec<_>>(),
            &proof.cmT.inputize(),
            &[proof.r],
        ]
        .concat();

        let snark_v = S::verify(&snark_vp, &public_input, &proof.snark_proof)
            .map_err(|e| Error::Other(e.to_string()))?;
        if !snark_v {
            return Err(Error::SNARKVerificationFail);
        }

        // 7.3. Verify the KZG proofs
        for ((cm, &c), pi) in U_final_commitments
            .iter()
            .zip(&proof.kzg_challenges)
            .zip(&proof.kzg_proofs)
        {
            // we're at the Ethereum EVM case, so the CS1 is KZG commitments
            CS1::verify_with_challenge(&cs_vp, c, cm, pi)?;
        }

        Ok(true)
    }
}

/// Prepares solidity calldata for calling the NovaDecider contract
#[allow(clippy::too_many_arguments)]
pub fn prepare_calldata(
    function_signature_check: [u8; 4],
    i: ark_bn254::Fr,
    z_0: Vec<ark_bn254::Fr>,
    z_i: Vec<ark_bn254::Fr>,
    running_instance: &CommittedInstance<ark_bn254::G1Projective>,
    incoming_instance: &CommittedInstance<ark_bn254::G1Projective>,
    proof: Proof<ark_bn254::G1Projective, KZG<'static, Bn254>, Groth16<Bn254>>,
) -> Result<Vec<u8>, Error> {
    Ok(vec![
        function_signature_check.to_vec(),
        i.into_bigint().to_bytes_be(), // i
        z_0.iter()
            .flat_map(|v| v.into_bigint().to_bytes_be())
            .collect::<Vec<u8>>(), // z_0
        z_i.iter()
            .flat_map(|v| v.into_bigint().to_bytes_be())
            .collect::<Vec<u8>>(), // z_i
        point_to_eth_format(running_instance.cmW.into_affine())?,
        point_to_eth_format(running_instance.cmE.into_affine())?,
        point_to_eth_format(incoming_instance.cmW.into_affine())?,
        point_to_eth_format(proof.cmT.into_affine())?, // cmT
        proof.r.into_bigint().to_bytes_be(),           // r
        point_to_eth_format(proof.snark_proof.a)?,     // pA
        point2_to_eth_format(proof.snark_proof.b)?,    // pB
        point_to_eth_format(proof.snark_proof.c)?,     // pC
        proof.kzg_challenges[0].into_bigint().to_bytes_be(), // challenge_W
        proof.kzg_challenges[1].into_bigint().to_bytes_be(), // challenge_E
        proof.kzg_proofs[0].eval.into_bigint().to_bytes_be(), // eval W
        proof.kzg_proofs[1].eval.into_bigint().to_bytes_be(), // eval E
        point_to_eth_format(proof.kzg_proofs[0].proof.into_affine())?, // W kzg_proof
        point_to_eth_format(proof.kzg_proofs[1].proof.into_affine())?, // E kzg_proof
    ]
    .concat())
}

fn point_to_eth_format<C: AffineRepr>(p: C) -> Result<Vec<u8>, Error>
where
    C::BaseField: PrimeField,
{
    // the encoding of the additive identity is [0, 0] on the EVM
    let zero_point = (&C::BaseField::zero(), &C::BaseField::zero());
    let (x, y) = p.xy().unwrap_or(zero_point);

    Ok([x.into_bigint().to_bytes_be(), y.into_bigint().to_bytes_be()].concat())
}
fn point2_to_eth_format(p: ark_bn254::G2Affine) -> Result<Vec<u8>, Error> {
    let zero_point = (&ark_bn254::Fq2::zero(), &ark_bn254::Fq2::zero());
    let (x, y) = p.xy().unwrap_or(zero_point);

    Ok([
        x.c1.into_bigint().to_bytes_be(),
        x.c0.into_bigint().to_bytes_be(),
        y.c1.into_bigint().to_bytes_be(),
        y.c0.into_bigint().to_bytes_be(),
    ]
    .concat())
}

#[cfg(test)]
pub mod tests {
    use ark_bn254::{constraints::GVar, Fr, G1Projective as Projective};
    use ark_grumpkin::{constraints::GVar as GVar2, Projective as Projective2};
    use std::time::Instant;

    use super::*;
    use crate::commitment::pedersen::Pedersen;
    use crate::folding::nova::{PreprocessorParam, ProverParams as NovaProverParams};
    use crate::folding::traits::CommittedInstanceOps;
    use crate::frontend::utils::CubicFCircuit;
    use crate::transcript::poseidon::poseidon_canonical_config;

    #[test]
    fn test_decider() {
        // use Nova as FoldingScheme
        type N = Nova<
            Projective,
            GVar,
            Projective2,
            GVar2,
            CubicFCircuit<Fr>,
            KZG<'static, Bn254>,
            Pedersen<Projective2>,
            false,
        >;
        type D = Decider<
            Projective,
            GVar,
            Projective2,
            GVar2,
            CubicFCircuit<Fr>,
            KZG<'static, Bn254>,
            Pedersen<Projective2>,
            Groth16<Bn254>, // here we define the Snark to use in the decider
            N,              // here we define the FoldingScheme to use
        >;

        let mut rng = rand::rngs::OsRng;
        let poseidon_config = poseidon_canonical_config::<Fr>();

        let F_circuit = CubicFCircuit::<Fr>::new(()).unwrap();
        let z_0 = vec![Fr::from(3_u32)];

        let preprocessor_param = PreprocessorParam::new(poseidon_config, F_circuit);
        let nova_params = N::preprocess(&mut rng, &preprocessor_param).unwrap();

        let start = Instant::now();
        let mut nova = N::init(&nova_params, F_circuit, z_0.clone()).unwrap();
        println!("Nova initialized, {:?}", start.elapsed());

        // prepare the Decider prover & verifier params
        let (decider_pp, decider_vp) = D::preprocess(&mut rng, nova_params, nova.clone()).unwrap();

        let start = Instant::now();
        nova.prove_step(&mut rng, vec![], None).unwrap();
        println!("prove_step, {:?}", start.elapsed());
        nova.prove_step(&mut rng, vec![], None).unwrap(); // do a 2nd step

        // decider proof generation
        let start = Instant::now();
        let proof = D::prove(rng, decider_pp, nova.clone()).unwrap();
        println!("Decider prove, {:?}", start.elapsed());

        // decider proof verification
        let start = Instant::now();
        let verified = D::verify(
            decider_vp.clone(),
            nova.i,
            nova.z_0.clone(),
            nova.z_i.clone(),
            &nova.U_i.get_commitments(),
            &nova.u_i.get_commitments(),
            &proof,
        )
        .unwrap();
        assert!(verified);
        println!("Decider verify, {:?}", start.elapsed());

        // decider proof verification using the deserialized data
        let verified = D::verify(
            decider_vp,
            nova.i,
            nova.z_0,
            nova.z_i,
            &nova.U_i.get_commitments(),
            &nova.u_i.get_commitments(),
            &proof,
        )
        .unwrap();
        assert!(verified);
    }

    // Test to check the serialization and deserialization of diverse Decider related parameters.
    // This test is the same test as `test_decider` but it serializes values and then uses the
    // deserialized values to continue the checks.
    #[test]
    fn test_decider_serialization() {
        // use Nova as FoldingScheme
        type N = Nova<
            Projective,
            GVar,
            Projective2,
            GVar2,
            CubicFCircuit<Fr>,
            KZG<'static, Bn254>,
            Pedersen<Projective2>,
            false,
        >;
        type D = Decider<
            Projective,
            GVar,
            Projective2,
            GVar2,
            CubicFCircuit<Fr>,
            KZG<'static, Bn254>,
            Pedersen<Projective2>,
            Groth16<Bn254>, // here we define the Snark to use in the decider
            N,              // here we define the FoldingScheme to use
        >;

        let mut rng = rand::rngs::OsRng;
        let poseidon_config = poseidon_canonical_config::<Fr>();

        let F_circuit = CubicFCircuit::<Fr>::new(()).unwrap();
        let z_0 = vec![Fr::from(3_u32)];

        let preprocessor_param = PreprocessorParam::new(poseidon_config, F_circuit);
        let nova_params = N::preprocess(&mut rng, &preprocessor_param).unwrap();

        let start = Instant::now();
        let nova = N::init(&nova_params, F_circuit, z_0.clone()).unwrap();
        println!("Nova initialized, {:?}", start.elapsed());

        // prepare the Decider prover & verifier params
        let (decider_pp, decider_vp) =
            D::preprocess(&mut rng, nova_params.clone(), nova.clone()).unwrap();

        // serialize the Nova params. These params are the trusted setup of the commitment schemes used
        // (ie. KZG & Pedersen in this case)
        let mut nova_pp_serialized = vec![];
        nova_params
            .0
            .serialize_compressed(&mut nova_pp_serialized)
            .unwrap();
        let mut nova_vp_serialized = vec![];
        nova_params
            .1
            .serialize_compressed(&mut nova_vp_serialized)
            .unwrap();
        // deserialize the Nova params. This would be done by the client reading from a file
        let nova_pp_deserialized = NovaProverParams::<
            Projective,
            Projective2,
            KZG<'static, Bn254>,
            Pedersen<Projective2>,
        >::deserialize_compressed(
            &mut nova_pp_serialized.as_slice()
        )
        .unwrap();
        let nova_vp_deserialized = <N as FoldingScheme<
            Projective,
            Projective2,
            CubicFCircuit<Fr>,
        >>::vp_deserialize_with_mode(
            &mut nova_vp_serialized.as_slice(),
            ark_serialize::Compress::Yes,
            ark_serialize::Validate::Yes,
            (), // fcircuit_params
        )
        .unwrap();

        // initialize nova again, but from the deserialized parameters
        let nova_params = (nova_pp_deserialized, nova_vp_deserialized);
        let mut nova = N::init(&nova_params, F_circuit, z_0).unwrap();

        let start = Instant::now();
        nova.prove_step(&mut rng, vec![], None).unwrap();
        println!("prove_step, {:?}", start.elapsed());
        nova.prove_step(&mut rng, vec![], None).unwrap(); // do a 2nd step

        // decider proof generation
        let start = Instant::now();
        let proof = D::prove(rng, decider_pp, nova.clone()).unwrap();
        println!("Decider prove, {:?}", start.elapsed());

        // decider proof verification
        let start = Instant::now();
        let verified = D::verify(
            decider_vp.clone(),
            nova.i,
            nova.z_0.clone(),
            nova.z_i.clone(),
            &nova.U_i.get_commitments(),
            &nova.u_i.get_commitments(),
            &proof,
        )
        .unwrap();
        assert!(verified);
        println!("Decider verify, {:?}", start.elapsed());

        // The rest of this test will serialize the data and deserialize it back, and use it to
        // verify the proof:

        // serialize the verifier_params, proof and public inputs
        let mut decider_vp_serialized = vec![];
        decider_vp
            .serialize_compressed(&mut decider_vp_serialized)
            .unwrap();
        let mut proof_serialized = vec![];
        proof.serialize_compressed(&mut proof_serialized).unwrap();
        // serialize the public inputs in a single packet
        let mut public_inputs_serialized = vec![];
        nova.i
            .serialize_compressed(&mut public_inputs_serialized)
            .unwrap();
        nova.z_0
            .serialize_compressed(&mut public_inputs_serialized)
            .unwrap();
        nova.z_i
            .serialize_compressed(&mut public_inputs_serialized)
            .unwrap();

        // deserialize back the verifier_params, proof and public inputs
        let decider_vp_deserialized =
            VerifierParam::<
                Projective,
                <KZG<'static, Bn254> as CommitmentScheme<Projective>>::VerifierParams,
                <Groth16<Bn254> as SNARK<Fr>>::VerifyingKey,
            >::deserialize_compressed(&mut decider_vp_serialized.as_slice())
            .unwrap();
        let proof_deserialized =
            Proof::<Projective, KZG<'static, Bn254>, Groth16<Bn254>>::deserialize_compressed(
                &mut proof_serialized.as_slice(),
            )
            .unwrap();

        // deserialize the public inputs from the single packet 'public_inputs_serialized'
        let mut reader = public_inputs_serialized.as_slice();
        let i_deserialized = Fr::deserialize_compressed(&mut reader).unwrap();
        let z_0_deserialized = Vec::<Fr>::deserialize_compressed(&mut reader).unwrap();
        let z_i_deserialized = Vec::<Fr>::deserialize_compressed(&mut reader).unwrap();

        // decider proof verification using the deserialized data
        let verified = D::verify(
            decider_vp_deserialized,
            i_deserialized,
            z_0_deserialized,
            z_i_deserialized,
            &nova.U_i.get_commitments(),
            &nova.u_i.get_commitments(),
            &proof_deserialized,
        )
        .unwrap();
        assert!(verified);
    }
}
