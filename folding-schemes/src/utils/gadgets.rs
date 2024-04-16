use ark_ff::PrimeField;
use ark_r1cs_std::{
    alloc::{AllocVar, AllocationMode},
    fields::{fp::FpVar, FieldVar},
    R1CSVar,
};
use ark_relations::r1cs::{Namespace, SynthesisError};
use core::{borrow::Borrow, marker::PhantomData};

use crate::utils::vec::SparseMatrix;

pub trait MatrixGadget<FV> {
    fn mul_vector(&self, v: &[FV]) -> Result<Vec<FV>, SynthesisError>;
}

pub trait VectorGadget<FV> {
    fn add(&self, other: &Self) -> Result<Vec<FV>, SynthesisError>;

    fn mul_scalar(&self, other: &FV) -> Result<Vec<FV>, SynthesisError>;

    fn hadamard(&self, other: &Self) -> Result<Vec<FV>, SynthesisError>;
}

impl<F: PrimeField> VectorGadget<FpVar<F>> for [FpVar<F>] {
    fn add(&self, other: &Self) -> Result<Vec<FpVar<F>>, SynthesisError> {
        if self.len() != other.len() {
            return Err(SynthesisError::Unsatisfiable);
        }
        Ok(self.iter().zip(other.iter()).map(|(a, b)| a + b).collect())
    }

    fn mul_scalar(&self, c: &FpVar<F>) -> Result<Vec<FpVar<F>>, SynthesisError> {
        Ok(self.iter().map(|a| a * c).collect())
    }

    fn hadamard(&self, other: &Self) -> Result<Vec<FpVar<F>>, SynthesisError> {
        if self.len() != other.len() {
            return Err(SynthesisError::Unsatisfiable);
        }
        Ok(self.iter().zip(other.iter()).map(|(a, b)| a * b).collect())
    }
}

#[derive(Debug, Clone)]
pub struct SparseMatrixVar<F: PrimeField, CF: PrimeField, FV: AllocVar<F, CF>> {
    _f: PhantomData<F>,
    _cf: PhantomData<CF>,
    _fv: PhantomData<FV>,
    pub n_rows: usize,
    pub n_cols: usize,
    // same format as the native SparseMatrix (which follows ark_relations::r1cs::Matrix format
    pub coeffs: Vec<Vec<(FV, usize)>>,
}

impl<F, CF, FV> AllocVar<SparseMatrix<F>, CF> for SparseMatrixVar<F, CF, FV>
where
    F: PrimeField,
    CF: PrimeField,
    FV: AllocVar<F, CF>,
{
    fn new_variable<T: Borrow<SparseMatrix<F>>>(
        cs: impl Into<Namespace<CF>>,
        f: impl FnOnce() -> Result<T, SynthesisError>,
        mode: AllocationMode,
    ) -> Result<Self, SynthesisError> {
        f().and_then(|val| {
            let cs = cs.into();

            let mut coeffs: Vec<Vec<(FV, usize)>> = Vec::new();
            for row in val.borrow().coeffs.iter() {
                let mut rowVar: Vec<(FV, usize)> = Vec::new();
                for &(value, col_i) in row.iter() {
                    let coeffVar = FV::new_variable(cs.clone(), || Ok(value), mode)?;
                    rowVar.push((coeffVar, col_i));
                }
                coeffs.push(rowVar);
            }

            Ok(Self {
                _f: PhantomData,
                _cf: PhantomData,
                _fv: PhantomData,
                n_rows: val.borrow().n_rows,
                n_cols: val.borrow().n_cols,
                coeffs,
            })
        })
    }
}

impl<F: PrimeField> MatrixGadget<FpVar<F>> for SparseMatrixVar<F, F, FpVar<F>> {
    fn mul_vector(&self, v: &[FpVar<F>]) -> Result<Vec<FpVar<F>>, SynthesisError> {
        Ok(self
            .coeffs
            .iter()
            .map(|row| {
                let products = row
                    .iter()
                    .map(|(value, col_i)| value * &v[*col_i])
                    .collect::<Vec<_>>();
                if products.is_constant() {
                    FpVar::constant(products.value().unwrap_or_default().into_iter().sum())
                } else {
                    products.iter().sum()
                }
            })
            .collect())
    }
}
