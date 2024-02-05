use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::PrimeField;
use ark_r1cs_std::fields::nonnative::{params::OptimizationType, AllocatedNonNativeFieldVar};
use ark_r1cs_std::{
    alloc::{AllocVar, AllocationMode},
    fields::{fp::FpVar, nonnative::NonNativeFieldVar},
    ToConstraintFieldGadget,
};
use ark_relations::r1cs::{Namespace, SynthesisError};
use ark_std::{One, Zero};
use core::borrow::Borrow;

/// NonNativeAffineVar represents an elliptic curve point in Affine represenation in the non-native
/// field, over the constraint field. It is not intended to perform operations, but just to contain
/// the affine coordinates in order to perform hash operations of the point.
#[derive(Debug, Clone)]
pub struct NonNativeAffineVar<F: PrimeField> {
    pub x: Vec<FpVar<F>>,
    pub y: Vec<FpVar<F>>,
}

impl<C> AllocVar<C, C::ScalarField> for NonNativeAffineVar<C::ScalarField>
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
            let zero_point = (&C::BaseField::zero(), &C::BaseField::one());
            let xy = affine.xy().unwrap_or(zero_point);

            let x = NonNativeFieldVar::<C::BaseField, C::ScalarField>::new_variable(
                cs.clone(),
                || Ok(xy.0),
                mode,
            )?
            .to_constraint_field()?;
            let y = NonNativeFieldVar::<C::BaseField, C::ScalarField>::new_variable(
                cs.clone(),
                || Ok(xy.1),
                mode,
            )?
            .to_constraint_field()?;

            Ok(Self { x, y })
        })
    }
}

/// Wrapper on top of [`point_to_nonnative_limbs_custom_opt`] which always uses
/// [`OptimizationType::Weight`].
#[allow(clippy::type_complexity)]
pub fn point_to_nonnative_limbs<C: CurveGroup>(
    p: C,
) -> Result<(Vec<C::ScalarField>, Vec<C::ScalarField>), SynthesisError>
where
    <C as ark_ec::CurveGroup>::BaseField: ark_ff::PrimeField,
{
    point_to_nonnative_limbs_custom_opt(p, OptimizationType::Weight)
}

/// Used to compute (outside the circuit) the limbs representation of a point that matches the one
/// used in-circuit, and in particular this method allows to specify which [`OptimizationType`] to
/// use.
#[allow(clippy::type_complexity)]
pub fn point_to_nonnative_limbs_custom_opt<C: CurveGroup>(
    p: C,
    optimization_type: OptimizationType,
) -> Result<(Vec<C::ScalarField>, Vec<C::ScalarField>), SynthesisError>
where
    <C as ark_ec::CurveGroup>::BaseField: ark_ff::PrimeField,
{
    let affine = p.into_affine();
    if affine.is_zero() {
        let x =
            AllocatedNonNativeFieldVar::<C::BaseField, C::ScalarField>::get_limbs_representations(
                &C::BaseField::zero(),
                optimization_type,
            )?;
        let y =
            AllocatedNonNativeFieldVar::<C::BaseField, C::ScalarField>::get_limbs_representations(
                &C::BaseField::one(),
                optimization_type,
            )?;
        return Ok((x, y));
    }

    let (x, y) = affine.xy().unwrap();
    let x = AllocatedNonNativeFieldVar::<C::BaseField, C::ScalarField>::get_limbs_representations(
        x,
        optimization_type,
    )?;
    let y = AllocatedNonNativeFieldVar::<C::BaseField, C::ScalarField>::get_limbs_representations(
        y,
        optimization_type,
    )?;
    Ok((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_pallas::{Fr, Projective};
    use ark_r1cs_std::{alloc::AllocVar, R1CSVar};
    use ark_relations::r1cs::ConstraintSystem;
    use ark_std::{UniformRand, Zero};

    #[test]
    fn test_alloc_nonnativeaffinevar() {
        let cs = ConstraintSystem::<Fr>::new_ref();

        // dealing with the 'zero' point should not panic when doing the unwrap
        let p = Projective::zero();
        NonNativeAffineVar::<Fr>::new_witness(cs.clone(), || Ok(p)).unwrap();

        // check that point_to_nonnative_limbs returns the expected values
        let mut rng = ark_std::test_rng();
        let p = Projective::rand(&mut rng);
        let pVar = NonNativeAffineVar::<Fr>::new_witness(cs.clone(), || Ok(p)).unwrap();
        let (x, y) = point_to_nonnative_limbs(p).unwrap();
        assert_eq!(pVar.x.value().unwrap(), x);
        assert_eq!(pVar.y.value().unwrap(), y);
    }
}
