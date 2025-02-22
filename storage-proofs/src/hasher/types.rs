use bellperson::gadgets::{boolean, num};
use bellperson::{ConstraintSystem, SynthesisError};
use fil_sapling_crypto::jubjub::JubjubEngine;
use merkletree::hash::{Algorithm as LightAlgorithm, Hashable as LightHashable};
use merkletree::merkle::Element;
use paired::bls12_381::{Fr, FrRepr};
use serde::de::DeserializeOwned;
use serde::ser::Serialize;

use crate::error::Result;

pub trait Domain:
    Ord
    + Copy
    + Clone
    + AsRef<[u8]>
    + Default
    + ::std::fmt::Debug
    + Eq
    + Send
    + Sync
    + From<Fr>
    + From<FrRepr>
    + Into<Fr>
    + Serialize
    + DeserializeOwned
    + Element
    + std::hash::Hash
{
    fn serialize(&self) -> Vec<u8>;
    fn into_bytes(&self) -> Vec<u8>;
    fn try_from_bytes(raw: &[u8]) -> Result<Self>;
    /// Write itself into the given slice, LittleEndian bytes.
    fn write_bytes(&self, _: &mut [u8]) -> Result<()>;

    fn random<R: rand::RngCore>(rng: &mut R) -> Self;
}

pub trait HashFunction<T: Domain>:
    Clone + ::std::fmt::Debug + Send + Sync + LightAlgorithm<T>
{
    fn hash(data: &[u8]) -> T;

    fn hash_leaf(data: &dyn LightHashable<Self>) -> T {
        let mut a = Self::default();
        data.hash(&mut a);
        let item_hash = a.hash();
        a.leaf(item_hash)
    }

    fn hash_single_node(data: &dyn LightHashable<Self>) -> T {
        let mut a = Self::default();
        data.hash(&mut a);
        a.hash()
    }

    fn hash_leaf_circuit<E: JubjubEngine, CS: ConstraintSystem<E>>(
        cs: CS,
        left: &[boolean::Boolean],
        right: &[boolean::Boolean],
        height: usize,
        params: &E::Params,
    ) -> std::result::Result<num::AllocatedNum<E>, SynthesisError>;

    fn hash_circuit<E: JubjubEngine, CS: ConstraintSystem<E>>(
        cs: CS,
        bits: &[boolean::Boolean],
        params: &E::Params,
    ) -> std::result::Result<num::AllocatedNum<E>, SynthesisError>;
}

pub trait Hasher: Clone + ::std::fmt::Debug + Eq + Default + Send + Sync {
    type Domain: Domain + LightHashable<Self::Function> + AsRef<Self::Domain>;
    type Function: HashFunction<Self::Domain>;

    fn create_label(data: &[u8], m: usize) -> Self::Domain;
    fn sloth_encode(key: &Self::Domain, ciphertext: &Self::Domain) -> Self::Domain;
    fn sloth_decode(key: &Self::Domain, ciphertext: &Self::Domain) -> Self::Domain;

    fn name() -> String;
}
