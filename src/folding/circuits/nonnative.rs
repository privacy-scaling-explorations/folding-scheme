use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;
use ark_r1cs_std::fields::nonnative::{params::OptimizationType, AllocatedNonNativeFieldVar};
use ark_r1cs_std::{
    alloc::{AllocVar, AllocationMode},
    fields::nonnative::NonNativeFieldVar,
};
use ark_relations::r1cs::{Namespace, SynthesisError};
use core::borrow::Borrow;

/// NonNativeAffineVar represents an elliptic curve point in Affine represenation in the non-native
/// field. It is not intended to perform operations, but just to contain the affine coordinates in
/// order to perform hash operations of the point.
#[derive(Debug, Clone)]
pub struct NonNativeAffineVar<F: PrimeField, CF: PrimeField> {
    pub x: NonNativeFieldVar<F, CF>,
    pub y: NonNativeFieldVar<F, CF>,
}

impl<C> AllocVar<C, C::ScalarField> for NonNativeAffineVar<C::BaseField, C::ScalarField>
where
    C: CurveGroup,
    <C as ark_ec::CurveGroup>::BaseField: ark_ff::PrimeField,
{
    fn new_variable<T: Borrow<C>>(
        cs: impl Into<Namespace<C::ScalarField>>,
        f: impl FnOnce() -> Result<T, SynthesisError>,
        mode: AllocationMode,
    ) -> Result<Self, SynthesisError> {
        f().and_then(|val| {
            let cs = cs.into();

            let affine = val.borrow().into_affine();
            let xy = affine.xy().unwrap();
            let x = NonNativeFieldVar::<C::BaseField, C::ScalarField>::new_variable(
                cs.clone(),
                || Ok(xy.0),
                mode,
            )?;
            let y = NonNativeFieldVar::<C::BaseField, C::ScalarField>::new_variable(
                cs.clone(),
                || Ok(xy.1),
                mode,
            )?;

            Ok(Self { x, y })
        })
    }
}

/// point_to_nonnative_limbs is used to return (outside the circuit) the limbs representation that
/// matches the one used in-circuit.
#[allow(clippy::type_complexity)]
pub fn point_to_nonnative_limbs<C: CurveGroup>(
    p: C,
) -> Result<(Vec<C::ScalarField>, Vec<C::ScalarField>), SynthesisError>
where
    <C as ark_ec::CurveGroup>::BaseField: ark_ff::PrimeField,
{
    let affine = p.into_affine();
    let (x, y) = affine.xy().unwrap();
    let x = AllocatedNonNativeFieldVar::<C::BaseField, C::ScalarField>::get_limbs_representations(
        x,
        OptimizationType::Weight,
    )?;
    let y = AllocatedNonNativeFieldVar::<C::BaseField, C::ScalarField>::get_limbs_representations(
        y,
        OptimizationType::Weight,
    )?;
    Ok((x, y))
}
