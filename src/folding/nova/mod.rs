/// Implements the scheme described in [Nova](https://eprint.iacr.org/2021/370.pdf)
use ark_crypto_primitives::{
    crh::{poseidon::CRH, CRHScheme},
    sponge::{poseidon::PoseidonConfig, Absorb},
};
use ark_ec::{CurveGroup, Group};
use ark_std::fmt::Debug;
use ark_std::{One, Zero};

use crate::folding::circuits::nonnative::point_to_nonnative_limbs;
use crate::pedersen::{Params as PedersenParams, Pedersen};
use crate::Error;

pub mod circuits;
pub mod nifs;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CommittedInstance<C: CurveGroup> {
    pub cmE: C,
    pub u: C::ScalarField,
    pub cmW: C,
    pub x: Vec<C::ScalarField>,
}

impl<C: CurveGroup> CommittedInstance<C>
where
    <C as Group>::ScalarField: Absorb,
    <C as ark_ec::CurveGroup>::BaseField: ark_ff::PrimeField,
{
    pub fn empty() -> Self {
        CommittedInstance {
            cmE: C::zero(),
            u: C::ScalarField::one(),
            cmW: C::zero(),
            x: Vec::new(),
        }
    }

    /// hash implements the committed instance hash compatible with the gadget implemented in
    /// nova/circuits.rs::CommittedInstanceVar.hash.
    /// Returns `H(i, z_0, z_i, U_i)`, where `i` can be `i` but also `i+1`, and `U` is the
    /// `CommittedInstance`.
    pub fn hash(
        &self,
        poseidon_config: &PoseidonConfig<C::ScalarField>,
        i: C::ScalarField,
        z_0: C::ScalarField,
        z_i: C::ScalarField,
    ) -> Result<C::ScalarField, Error> {
        let (cmE_x, cmE_y) = point_to_nonnative_limbs::<C>(self.cmE)?;
        let (cmW_x, cmW_y) = point_to_nonnative_limbs::<C>(self.cmW)?;

        Ok(CRH::<C::ScalarField>::evaluate(
            poseidon_config,
            vec![
                vec![i, z_0, z_i, self.u],
                self.x.clone(),
                cmE_x,
                cmE_y,
                cmW_x,
                cmW_y,
            ]
            .concat(),
        )
        .unwrap())
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
        Self {
            E: vec![C::ScalarField::zero(); e_len],
            rE: C::ScalarField::one(),
            W: w,
            rW: C::ScalarField::one(),
        }
    }
    pub fn commit(
        &self,
        params: &PedersenParams<C>,
        x: Vec<C::ScalarField>,
    ) -> CommittedInstance<C> {
        let cmE = Pedersen::commit(params, &self.E, &self.rE);
        let cmW = Pedersen::commit(params, &self.W, &self.rW);
        CommittedInstance {
            cmE,
            u: C::ScalarField::one(),
            cmW,
            x,
        }
    }
}
