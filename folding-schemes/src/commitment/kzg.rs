//! KZG polynomial commitment scheme implementation.
//!
//! This module provides an implementation of the KZG polynomial commitment scheme that implements
//! the [`CommitmentScheme`] trait. The implementation is adapted from arkworks' KZG10 implementation
//! to work with just `CurveGroup` rather than requiring full pairing operations for the prover.
//!
//! # Overview
//!
//! The KZG polynomial commitment scheme allows proving evaluations of committed polynomials.
//! This implementation:
//!
//! - Adapts the arkworks KZG10 implementation to work with the [`CommitmentScheme`] trait
//! - Separates prover operations to only require `CurveGroup` operations, not full pairings  
//! - Currently only supports non-hiding commitments

use ark_ec::{pairing::Pairing, CurveGroup, VariableBaseMSM};
use ark_ff::PrimeField;
use ark_poly::{
    univariate::{DenseOrSparsePolynomial, DensePolynomial},
    DenseUVPolynomial, Polynomial,
};
use ark_poly_commit::kzg10::{
    Commitment as KZG10Commitment, Proof as KZG10Proof, VerifierKey, KZG10,
};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize, Valid};
use ark_std::rand::RngCore;
use ark_std::{borrow::Cow, fmt::Debug};
use ark_std::{One, Zero};
use core::marker::PhantomData;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use super::CommitmentScheme;
use crate::transcript::Transcript;
use crate::utils::vec::poly_from_vec;
use crate::Error;

/// Prover key containing powers of group elements needed for KZG polynomial commitments.
///
/// This is similar to `ark_poly_commit::kzg10::Powers` but depends only on `CurveGroup`
/// rather than requiring pairing operations.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct ProverKey<'a, C: CurveGroup> {
    /// Group elements of the form `β^i G`, for different values of `i`.
    /// These are used to commit to polynomial coefficients.
    pub powers_of_g: Cow<'a, [C::Affine]>,
}

impl<'a, C: CurveGroup> CanonicalSerialize for ProverKey<'a, C> {
    fn serialize_with_mode<W: std::io::prelude::Write>(
        &self,
        mut writer: W,
        compress: ark_serialize::Compress,
    ) -> Result<(), ark_serialize::SerializationError> {
        self.powers_of_g.serialize_with_mode(&mut writer, compress)
    }

    fn serialized_size(&self, compress: ark_serialize::Compress) -> usize {
        self.powers_of_g.serialized_size(compress)
    }
}

impl<'a, C: CurveGroup> CanonicalDeserialize for ProverKey<'a, C> {
    fn deserialize_with_mode<R: std::io::prelude::Read>(
        reader: R,
        compress: ark_serialize::Compress,
        validate: ark_serialize::Validate,
    ) -> Result<Self, ark_serialize::SerializationError> {
        let powers_of_g_vec = Vec::deserialize_with_mode(reader, compress, validate)?;
        Ok(ProverKey {
            powers_of_g: ark_std::borrow::Cow::Owned(powers_of_g_vec),
        })
    }
}

impl<'a, C: CurveGroup> Valid for ProverKey<'a, C> {
    fn check(&self) -> Result<(), ark_serialize::SerializationError> {
        match self.powers_of_g.clone() {
            Cow::Borrowed(powers) => powers.to_vec().check(),
            Cow::Owned(powers) => powers.check(),
        }
    }
}

/// Proof of polynomial evaluation at a point.
///
/// Contains both the claimed evaluation and a KZG proof element.
#[derive(Debug, Clone, Default, Eq, PartialEq, CanonicalSerialize, CanonicalDeserialize)]
pub struct Proof<C: CurveGroup> {
    /// The claimed evaluation value f(z)
    pub eval: C::ScalarField,

    /// The proof element π = (f(X) - f(z))/(X - z)
    pub proof: C,
}

/// KZG polynomial commitment scheme implementation.
///
/// This implements the [`CommitmentScheme`] trait for KZG polynomial commitments.
/// The type parameter `H` controls whether hiding commitments are used (currently unsupported).
///
/// # Type Parameters
///
/// * `'a` - Lifetime of the prover parameters
/// * `E` - The pairing engine
/// * `H` - Whether hiding commitments are used (must be false currently)
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct KZG<'a, E: Pairing, const H: bool = false> {
    /// Inner lifetime accounting
    _a: PhantomData<&'a ()>,
    /// Inner [`Pairing`] friendly curve
    _e: PhantomData<E>,
}

/*
TODO (autoparallel): Okay, I'm noticing something here, basically I think that there should likely be two implementations for `CommitmentScheme<G>`,
one that is hiding, and one that is not (as opposed to the const generic `H` in the trait itself). We could have `HidingCommitmentScheme: CommitmentScheme`.
If impl `CommitmentScheme<G> for KZG<'a, E, H>`, then we can impl `HidingCommitmentScheme for KZG<'a, E, true>`. The implementation of `HidingCommitmentScheme`
would be super straight forward as it would just add the blinding factor to the output of the "super" `CommitmentScheme` `commit` and `prove` methods. Then those
methods on `CommitmentScheme` do not have to take in `blind: Option<E::ScalarField>` or the `dyn Rng`.
*/

impl<'a, E, const H: bool> CommitmentScheme<E::G1, H> for KZG<'a, E, H>
where
    E: Pairing,
{
    type ProverParams = ProverKey<'a, E::G1>;
    type VerifierParams = VerifierKey<E>;
    type Proof = Proof<E::G1>;
    type ProverChallenge = E::ScalarField;
    type Challenge = E::ScalarField;

    fn setup(
        mut rng: impl RngCore,
        len: usize,
    ) -> Result<(Self::ProverParams, Self::VerifierParams), Error> {
        let len = len.next_power_of_two();
        // TODO (autoparallel): This `expect` will panic when the function itself is capable of returning an error.
        let universal_params =
            KZG10::<E, DensePolynomial<E::ScalarField>>::setup(len, false, &mut rng)
                .expect("Setup failed");
        let powers_of_g = universal_params.powers_of_g[..=len].to_vec();
        let powers = ProverKey::<E::G1> {
            powers_of_g: ark_std::borrow::Cow::Owned(powers_of_g),
        };
        let vk = VerifierKey {
            g: universal_params.powers_of_g[0],
            gamma_g: universal_params.powers_of_gamma_g[&0],
            h: universal_params.h,
            beta_h: universal_params.beta_h,
            prepared_h: universal_params.prepared_h.clone(),
            prepared_beta_h: universal_params.prepared_beta_h.clone(),
        };
        Ok((powers, vk))
    }

    /// commit implements the [`CommitmentScheme`] commit interface, adapting the implementation from
    /// https://github.com/arkworks-rs/poly-commit/tree/c724fa666e935bbba8db5a1421603bab542e15ab/poly-commit/src/kzg10/mod.rs#L178
    /// with the main difference being the removal of the blinding factors and the no-dependency to
    /// the Pairing trait.
    fn commit(
        params: &Self::ProverParams,
        v: &[E::ScalarField],
        _blind: &E::ScalarField,
    ) -> Result<E::G1, Error> {
        // TODO (autoparallel): awk to use `_` prefix here.
        if !_blind.is_zero() || H {
            return Err(Error::NotSupportedYet("hiding".to_string()));
        }

        let polynomial = poly_from_vec(v.to_vec())?;
        check_degree_is_too_large(polynomial.degree(), params.powers_of_g.len())?;

        let (num_leading_zeros, plain_coeffs) =
            skip_first_zero_coeffs_and_convert_to_bigints(&polynomial);
        let commitment = <E::G1 as VariableBaseMSM>::msm_bigint(
            &params.powers_of_g[num_leading_zeros..],
            &plain_coeffs,
        );
        Ok(commitment)
    }

    /// prove implements the [`CommitmentScheme`] prove interface, adapting the implementation from
    /// <https://github.com/arkworks-rs/poly-commit/tree/c724fa666e935bbba8db5a1421603bab542e15ab/poly-commit/src/kzg10/mod.rs#L307>
    /// with the main difference being the removal of the blinding factors and the no-dependency to
    /// the Pairing trait.
    fn prove(
        params: &Self::ProverParams,
        transcript: &mut impl Transcript<E::ScalarField>,
        cm: &E::G1,
        v: &[E::ScalarField],
        _blind: &E::ScalarField,
        _rng: Option<&mut dyn RngCore>,
    ) -> Result<Self::Proof, Error> {
        transcript.absorb_nonnative(cm);
        let challenge = transcript.get_challenge();
        // TODO (autoparallel): awk to use `_` prefix here.
        Self::prove_with_challenge(params, challenge, v, _blind, _rng)
    }

    fn prove_with_challenge(
        params: &Self::ProverParams,
        challenge: Self::ProverChallenge,
        v: &[E::ScalarField],
        _blind: &E::ScalarField,
        _rng: Option<&mut dyn RngCore>,
    ) -> Result<Self::Proof, Error> {
        // TODO (autoparallel): awk to use `_` prefix here.
        if !_blind.is_zero() || H {
            return Err(Error::NotSupportedYet("hiding".to_string()));
        }

        let polynomial = poly_from_vec(v.to_vec())?;
        check_degree_is_too_large(polynomial.degree(), params.powers_of_g.len())?;

        // Compute q(x) = (p(x) - p(z)) / (x-z). Observe that this quotient does not change with z
        // because p(z) is the remainder term. We can therefore omit p(z) when computing the
        // quotient.
        let divisor = DensePolynomial::<E::ScalarField>::from_coefficients_vec(vec![
            -challenge,
            E::ScalarField::one(),
        ]);
        let (witness_poly, remainder_poly) = DenseOrSparsePolynomial::from(&polynomial)
            .divide_with_q_and_r(&DenseOrSparsePolynomial::from(&divisor))
            // the panic inside `divide_with_q_and_r` should never be reached, since the divisor
            // polynomial is constructed right before and is set to not be zero. And the `.unwrap`
            // should not give an error.
            .unwrap();

        let eval = if remainder_poly.is_zero() {
            E::ScalarField::zero()
        } else {
            remainder_poly[0]
        };

        check_degree_is_too_large(witness_poly.degree(), params.powers_of_g.len())?;
        let (num_leading_zeros, witness_coeffs) =
            skip_first_zero_coeffs_and_convert_to_bigints(&witness_poly);
        let proof = <E::G1 as VariableBaseMSM>::msm_bigint(
            &params.powers_of_g[num_leading_zeros..],
            &witness_coeffs,
        );

        Ok(Proof { eval, proof })
    }

    fn verify(
        params: &Self::VerifierParams,
        transcript: &mut impl Transcript<E::ScalarField>,
        cm: &E::G1,
        proof: &Self::Proof,
    ) -> Result<(), Error> {
        transcript.absorb_nonnative(cm);
        let challenge = transcript.get_challenge();
        Self::verify_with_challenge(params, challenge, cm, proof)
    }

    fn verify_with_challenge(
        params: &Self::VerifierParams,
        challenge: Self::Challenge,
        cm: &E::G1,
        proof: &Self::Proof,
    ) -> Result<(), Error> {
        if H {
            return Err(Error::NotSupportedYet("hiding".to_string()));
        }

        // verify the KZG proof using arkworks method
        let v = KZG10::<E, DensePolynomial<E::ScalarField>>::check(
            params, // vk
            &KZG10Commitment(cm.into_affine()),
            challenge,
            proof.eval,
            &KZG10Proof::<E> {
                w: proof.proof.into_affine(),
                random_v: None,
            },
        )?;
        if !v {
            return Err(Error::CommitmentVerificationFail);
        }
        Ok(())
    }
}

/// Helper function to check if polynomial degree exceeds supported length
const fn check_degree_is_too_large(
    degree: usize,
    num_powers: usize,
) -> Result<(), ark_poly_commit::error::Error> {
    let num_coefficients = degree + 1;
    if num_coefficients > num_powers {
        Err(ark_poly_commit::error::Error::TooManyCoefficients {
            num_coefficients,
            num_powers,
        })
    } else {
        Ok(())
    }
}

/// Helper function to skip leading zero coefficients and convert to bigints
fn skip_first_zero_coeffs_and_convert_to_bigints<F: PrimeField, P: DenseUVPolynomial<F>>(
    p: &P,
) -> (usize, Vec<F::BigInt>) {
    let mut num_leading_zeros = 0;
    while num_leading_zeros < p.coeffs().len() && p.coeffs()[num_leading_zeros].is_zero() {
        num_leading_zeros += 1;
    }
    let coeffs = convert_to_bigints(&p.coeffs()[num_leading_zeros..]);
    (num_leading_zeros, coeffs)
}

/// Helper function to convert coefficients to bigints
fn convert_to_bigints<F: PrimeField>(p: &[F]) -> Vec<F::BigInt> {
    ark_std::cfg_iter!(p)
        .map(|s| s.into_bigint())
        .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use ark_bn254::{Bn254, Fr, G1Projective as G1};
    use ark_crypto_primitives::sponge::{poseidon::PoseidonSponge, CryptographicSponge};
    use ark_std::{test_rng, UniformRand};

    use super::*;
    use crate::transcript::poseidon::poseidon_canonical_config;

    #[test]
    fn test_kzg_commitment_scheme() -> Result<(), Error> {
        let mut rng = &mut test_rng();
        let poseidon_config = poseidon_canonical_config::<Fr>();
        let transcript_p = &mut PoseidonSponge::<Fr>::new(&poseidon_config);
        let transcript_v = &mut PoseidonSponge::<Fr>::new(&poseidon_config);

        let n = 10;
        let (pk, vk): (ProverKey<G1>, VerifierKey<Bn254>) = KZG::<Bn254>::setup(&mut rng, n)?;

        let v: Vec<Fr> = std::iter::repeat_with(|| Fr::rand(rng)).take(n).collect();
        let cm = KZG::<Bn254>::commit(&pk, &v, &Fr::zero())?;

        let proof = KZG::<Bn254>::prove(&pk, transcript_p, &cm, &v, &Fr::zero(), None)?;

        // verify the proof:
        KZG::<Bn254>::verify(&vk, transcript_v, &cm, &proof)?;
        Ok(())
    }
}
