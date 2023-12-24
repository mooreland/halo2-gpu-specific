use super::super::{
    circuit::Expression, ChallengeBeta, ChallengeGamma, ChallengeTheta, ChallengeX, Error,
    ProvingKey,
};
use super::Argument;
use crate::arithmetic::{batch_invert, eval_polynomial_st};
use crate::plonk::evaluation::{evaluate, evaluate_with_theta};
use crate::poly::Basis;
use crate::{
    arithmetic::{eval_polynomial, parallelize, BaseExt, CurveAffine, FieldExt},
    poly::{
        commitment::Params, multiopen::ProverQuery, Coeff, EvaluationDomain, ExtendedLagrangeCoeff,
        LagrangeCoeff, Polynomial, Rotation,
    },
    transcript::{EncodedChallenge, TranscriptWrite},
};
use ark_std::UniformRand;
use ark_std::{end_timer, start_timer};
use ff::PrimeField;
use group::{
    ff::{BatchInvert, Field},
    Curve,
};
use rand_core::RngCore;
use rayon::prelude::{
    IndexedParallelIterator, IntoParallelIterator, IntoParallelRefIterator,
    IntoParallelRefMutIterator, ParallelIterator, ParallelSliceMut,
};
use std::any::TypeId;
use std::convert::TryInto;
use std::num::ParseIntError;
use std::ops::Index;
use std::{
    collections::BTreeMap,
    iter,
    ops::{Mul, MulAssign},
};

#[derive(Debug)]
pub(in crate::plonk) struct Compressed<C: CurveAffine> {
    input_expression: Polynomial<C::Scalar, LagrangeCoeff>,
    shuffle_expression: Polynomial<C::Scalar, LagrangeCoeff>,
}

#[derive(Debug)]
pub(in crate::plonk) struct Committed<C: CurveAffine> {
    pub(in crate::plonk) product_poly: Polynomial<C::Scalar, Coeff>,
}

pub(in crate::plonk) struct Evaluated<C: CurveAffine> {
    constructed: Committed<C>,
}

impl<F: FieldExt> Argument<F> {
    /// Given a Shuffle with input expressions [A_0, A_1, ..., A_{m-1}] and table expressions
    /// [S_0, S_1, ..., S_{m-1}], this method
    /// - constructs A_compressed = \theta^{m-1} A_0 + theta^{m-2} A_1 + ... + \theta A_{m-2} + A_{m-1}
    ///   and S_compressed = \theta^{m-1} S_0 + theta^{m-2} S_1 + ... + \theta S_{m-2} + S_{m-1},
    pub(in crate::plonk) fn compress<'a, C>(
        &self,
        pk: &ProvingKey<C>,
        params: &Params<C>,
        theta: ChallengeTheta<C>,
        advice_values: &'a [Polynomial<C::Scalar, LagrangeCoeff>],
        fixed_values: &'a [Polynomial<C::Scalar, LagrangeCoeff>],
        instance_values: &'a [Polynomial<C::Scalar, LagrangeCoeff>],
    ) -> Result<Compressed<C>, Error>
    where
        C: CurveAffine<ScalarExt = F>,
        C::Curve: Mul<F, Output = C::Curve> + MulAssign<F>,
    {
        // Closure to get values of expressions and compress them
        let compress_expressions = |expressions: &[Expression<C::Scalar>]| {
            pk.vk.domain.lagrange_from_vec(evaluate_with_theta(
                expressions,
                params.n as usize,
                1,
                fixed_values,
                advice_values,
                instance_values,
                *theta,
            ))
        };

        // Get values of input expressions involved in the shuffle and compress them
        let input_expression = compress_expressions(&self.input_expressions);

        // Get values of table expressions involved in the shuffle and compress them
        let shuffle_expression = compress_expressions(&self.shuffle_expressions);
        Ok(Compressed {
            input_expression,
            shuffle_expression,
        })
    }
}

impl<C: CurveAffine> Compressed<C> {
    /// Given a Shuffle with input expressions, table expressions this method constructs the
    /// grand product polynomial over the shuffle. The grand product polynomial
    /// is used to populate the Product<C> struct. The Product<C> struct is
    /// added to the Shuffle and finally returned by the method.
    pub(in crate::plonk) fn commit_product(
        self,
        pk: &ProvingKey<C>,
        params: &Params<C>,
        gamma: ChallengeGamma<C>,
    ) -> Result<Vec<C::Scalar>, Error> {
        let blinding_factors = pk.vk.cs.blinding_factors();

        let mut shuffle_product = vec![C::Scalar::zero(); params.n as usize];
        // #[cfg(not(feature = "cuda"))]
        {
            // Denominator uses table expression
            parallelize(&mut shuffle_product, |shuffle_product, start| {
                for (shuffle_product, shuffle_value) in shuffle_product
                    .iter_mut()
                    .zip(self.shuffle_expression[start..].iter())
                {
                    *shuffle_product = *gamma + shuffle_value;
                }
            });

            // Batch invert to obtain the denominators for the product polynomials
            batch_invert(&mut shuffle_product);

            // Finish the computation of the entire fraction by computing the numerators of input expressions
            parallelize(&mut shuffle_product, |product, start| {
                for (i, product) in product.iter_mut().enumerate() {
                    let i = i + start;
                    *product *= &(self.input_expression[i] + &*gamma);
                }
            });
        }

        let z = iter::once(C::Scalar::one())
            .chain(shuffle_product)
            .scan(C::Scalar::one(), |state, cur| {
                *state *= &cur;
                Some(*state)
            })
            // Take all rows including the "last" row which should
            // be a boolean (and ideally 1, else soundness is broken)
            .take(params.n as usize - blinding_factors)
            .collect::<Vec<_>>();

        #[cfg(feature = "sanity-checks")]
        {
            // While in Lagrange basis, check that product is correctly constructed
            let u = (params.n as usize) - (blinding_factors + 1);
            // l_0(X) * (1 - z(X)) = 0
            assert_eq!(z[0], C::Scalar::one());
            // z(\omega X) (s'(X) + \gamma) - z(X)(a'(X)  + \gamma) =0
            for i in 0..u {
                let mut left = z[i + 1];
                let mut table_term = self.shuffle_expression[i];
                table_term += &(*gamma);
                left *= &(table_term);

                let mut right = z[i];
                let mut input_term = self.input_expression[i];
                input_term += &(*gamma);
                right *= &(input_term);

                assert_eq!(left, right);
            }

            // l_last(X) * (z(X)^2 - z(X)) = 0
            // Assertion will fail only when soundness is broken, in which
            // case this z[u] value will be zero. (bad!)
            assert_eq!(z[u], C::Scalar::one());
        }

        Ok(z)
    }
}

impl<C: CurveAffine> Committed<C> {
    pub(in crate::plonk) fn evaluate(
        self,
        pk: &ProvingKey<C>,
        x: ChallengeX<C>,
    ) -> (Evaluated<C>, Vec<C::ScalarExt>) {
        let domain = &pk.vk.domain;
        let x_next = domain.rotate_omega(*x, Rotation::next());

        let evals = vec![(&self.product_poly, *x), (&self.product_poly, x_next)]
            .into_par_iter()
            .map(|(a, b)| eval_polynomial_st(a, b))
            .collect();

        (Evaluated { constructed: self }, evals)
    }
}

impl<C: CurveAffine> Evaluated<C> {
    pub(in crate::plonk) fn open<'a>(
        &'a self,
        pk: &'a ProvingKey<C>,
        x: ChallengeX<C>,
    ) -> impl Iterator<Item = ProverQuery<'a, C>> + Clone {
        let x_next = pk.vk.domain.rotate_omega(*x, Rotation::next());

        iter::empty()
            // Open  product commitments at x
            .chain(Some(ProverQuery {
                point: *x,
                rotation: Rotation::cur(),
                poly: &self.constructed.product_poly,
            }))
            // Open  product commitments at x_next
            .chain(Some(ProverQuery {
                point: x_next,
                rotation: Rotation::next(),
                poly: &self.constructed.product_poly,
            }))
    }
}