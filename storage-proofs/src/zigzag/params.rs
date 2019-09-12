use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

use crate::drgporep;
use crate::drgraph::Graph;
use crate::error::Result;
use crate::hasher::{Domain, Hasher};
use crate::merkle::{MerkleProof, MerkleTree};
use crate::parameter_cache::ParameterSetMetadata;
use crate::util::data_at_node;
use crate::zigzag::{
    column::Column, column_proof::ColumnProof, encoding_proof::EncodingProof,
    graph::ZigZagBucketGraph, LayerChallenges,
};

pub type Tree<H> = MerkleTree<<H as Hasher>::Domain, <H as Hasher>::Function>;

#[derive(Debug)]
pub struct SetupParams {
    pub drg: drgporep::DrgParams,
    pub layer_challenges: LayerChallenges,
}

#[derive(Debug, Clone)]
pub struct PublicParams<H, G>
where
    H: Hasher,
    G: Graph<H> + ParameterSetMetadata,
{
    pub graph: G,
    pub layer_challenges: LayerChallenges,
    _h: PhantomData<H>,
}

impl<H, G> PublicParams<H, G>
where
    H: Hasher,
    G: Graph<H> + ParameterSetMetadata,
{
    pub fn new(graph: G, layer_challenges: LayerChallenges) -> Self {
        PublicParams {
            graph,
            layer_challenges,
            _h: PhantomData,
        }
    }
}

impl<H, G> ParameterSetMetadata for PublicParams<H, G>
where
    H: Hasher,
    G: Graph<H> + ParameterSetMetadata,
{
    fn identifier(&self) -> String {
        format!(
            "layered_drgporep::PublicParams{{ graph: {}, challenges: {:?} }}",
            self.graph.identifier(),
            self.layer_challenges,
        )
    }

    fn sector_size(&self) -> u64 {
        self.graph.sector_size()
    }
}

impl<'a, H, G> From<&'a PublicParams<H, G>> for PublicParams<H, G>
where
    H: Hasher,
    G: Graph<H> + ParameterSetMetadata,
{
    fn from(other: &PublicParams<H, G>) -> PublicParams<H, G> {
        PublicParams::new(other.graph.clone(), other.layer_challenges.clone())
    }
}

#[derive(Debug, Clone)]
pub struct PublicInputs<T: Domain> {
    pub replica_id: T,
    pub seed: Option<T>,
    pub tau: Tau<T>,
    pub k: Option<usize>,
}

impl<T: Domain> PublicInputs<T> {
    pub fn challenges(
        &self,
        layer_challenges: &LayerChallenges,
        leaves: usize,
        partition_k: Option<usize>,
    ) -> Vec<usize> {
        let k = partition_k.unwrap_or(0);

        if let Some(ref seed) = self.seed {
            layer_challenges.derive::<T>(leaves, &self.replica_id, seed, k as u8)
        } else {
            layer_challenges.derive::<T>(leaves, &self.replica_id, &self.tau.comm_r, k as u8)
        }
    }
}

pub struct PrivateInputs<H: Hasher> {
    pub p_aux: PersistentAux<H::Domain>,
    pub t_aux: TemporaryAux<H>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proof<H: Hasher> {
    #[serde(bound(
        serialize = "MerkleProof<H>: Serialize",
        deserialize = "MerkleProof<H>: Deserialize<'de>"
    ))]
    pub comm_d_proofs: Vec<MerkleProof<H>>,
    #[serde(bound(
        serialize = "MerkleProof<H>: Serialize, ColumnProof<H>: Serialize",
        deserialize = "MerkleProof<H>: Deserialize<'de>, ColumnProof<H>: Deserialize<'de>"
    ))]
    pub comm_r_last_proofs: Vec<(MerkleProof<H>, Vec<MerkleProof<H>>)>,
    #[serde(bound(
        serialize = "ReplicaColumnProof<H>: Serialize",
        deserialize = "ReplicaColumnProof<H>: Deserialize<'de>"
    ))]
    pub replica_column_proofs: Vec<ReplicaColumnProof<H>>,
    #[serde(bound(
        serialize = "EncodingProof<H>: Serialize",
        deserialize = "EncodingProof<H>: Deserialize<'de>"
    ))]
    /// Indexed by challenge. The encoding proof for layer 1.
    pub encoding_proof_1: Vec<EncodingProof<H>>,
    #[serde(bound(
        serialize = "EncodingProof<H>: Serialize",
        deserialize = "EncodingProof<H>: Deserialize<'de>"
    ))]
    /// Indexed first by challenge then by layer in 2..layers - 1.
    pub encoding_proofs: Vec<Vec<EncodingProof<H>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicaColumnProof<H: Hasher> {
    #[serde(bound(
        serialize = "ColumnProof<H>: Serialize",
        deserialize = "ColumnProof<H>: Deserialize<'de>"
    ))]
    pub c_x: ColumnProof<H>,
    #[serde(bound(
        serialize = "ColumnProof<H>: Serialize",
        deserialize = "ColumnProof<H>: Deserialize<'de>"
    ))]
    pub c_inv_x: ColumnProof<H>,
    #[serde(bound(
        serialize = "ColumnProof<H>: Serialize",
        deserialize = "ColumnProof<H>: Deserialize<'de>"
    ))]
    pub drg_parents: Vec<ColumnProof<H>>,
    #[serde(bound(
        serialize = "ColumnProof<H>: Serialize",
        deserialize = "ColumnProof<H>: Deserialize<'de>"
    ))]
    pub exp_parents_even: Vec<ColumnProof<H>>,
    #[serde(bound(
        serialize = "ColumnProof<H>: Serialize",
        deserialize = "ColumnProof<H>: Deserialize<'de>"
    ))]
    pub exp_parents_odd: Vec<ColumnProof<H>>,
}

impl<H: Hasher> Proof<H> {
    pub fn serialize(&self) -> Vec<u8> {
        unimplemented!();
    }
}

pub type TransformedLayers<H> = (
    Tau<<H as Hasher>::Domain>,
    PersistentAux<<H as Hasher>::Domain>,
    TemporaryAux<H>,
);

/// Tau for a single parition.
#[derive(Debug, Clone)]
pub struct Tau<D: Domain> {
    pub comm_d: D,
    pub comm_r: D,
}

#[derive(Debug, Clone)]
/// Stored along side the sector on disk.
pub struct PersistentAux<D: Domain> {
    pub comm_c: D,
    pub comm_r_last: D,
}

#[derive(Debug, Clone)]
pub struct TemporaryAux<H: Hasher> {
    /// The encoded nodes for 1..layers.
    pub encodings: Encodings<H>,
    pub tree_d: Tree<H>,
    pub tree_r_last: Tree<H>,
    pub tree_c: Tree<H>,
    /// E_i
    pub es: Vec<Vec<u8>>,
    /// O_i
    pub os: Vec<Vec<u8>>,
}

impl<H: Hasher> TemporaryAux<H> {
    pub fn encoding_at_layer(&self, layer: usize) -> &[u8] {
        self.encodings.encoding_at_layer(layer)
    }

    pub fn node_at_layer(&self, layer: usize, node_index: usize) -> Result<&[u8]> {
        self.encodings.node_at_layer(layer, node_index)
    }

    pub fn even_column(&self, column_index: usize) -> Result<Column<H>> {
        self.encodings.even_column(column_index)
    }

    pub fn odd_column(&self, column_index: usize) -> Result<Column<H>> {
        self.encodings.odd_column(column_index)
    }

    pub fn full_column(
        &self,
        graph: &ZigZagBucketGraph<H>,
        column_index: usize,
    ) -> Result<Column<H>> {
        self.encodings.full_column(graph, column_index)
    }
}

#[derive(Debug, Clone)]
pub struct Encodings<H: Hasher> {
    encodings: Vec<Vec<u8>>,
    _h: PhantomData<H>,
}

impl<H: Hasher> Encodings<H> {
    pub fn new(encodings: Vec<Vec<u8>>) -> Self {
        Encodings {
            encodings,
            _h: PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.encodings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.encodings.is_empty()
    }

    pub fn encoding_at_layer(&self, layer: usize) -> &[u8] {
        assert!(layer != 0, "Layer cannot be 0");
        assert!(
            layer < self.layers(),
            "Layer {} is not available (only {} layers available)",
            layer,
            self.layers()
        );

        let row_index = layer - 1;
        &self.encodings[row_index][..]
    }

    /// How many layers are available.
    fn layers(&self) -> usize {
        self.encodings.len() + 1
    }

    pub fn node_at_layer(&self, layer: usize, node_index: usize) -> Result<&[u8]> {
        let encoding = self.encoding_at_layer(layer);
        data_at_node(encoding, node_index)
    }

    pub fn even_column(&self, column_index: usize) -> Result<Column<H>> {
        let rows = (2..self.layers())
            .step_by(2)
            .map(|layer| H::Domain::try_from_bytes(self.node_at_layer(layer, column_index)?))
            .collect::<Result<_>>()?;

        Ok(Column::new_even(column_index, rows))
    }

    pub fn odd_column(&self, column_index: usize) -> Result<Column<H>> {
        let rows = (1..self.layers())
            .step_by(2)
            .map(|layer| H::Domain::try_from_bytes(self.node_at_layer(layer, column_index)?))
            .collect::<Result<_>>()?;

        Ok(Column::new_odd(column_index, rows))
    }

    pub fn full_column(
        &self,
        graph: &ZigZagBucketGraph<H>,
        column_index: usize,
    ) -> Result<Column<H>> {
        let inv_index = graph.inv_index(column_index);

        let rows = (1..self.layers())
            .map(|layer| {
                let x = if layer % 2 == 0 {
                    inv_index
                } else {
                    column_index
                };

                H::Domain::try_from_bytes(self.node_at_layer(layer, x)?)
            })
            .collect::<Result<_>>()?;

        Ok(Column::new_all(column_index, rows))
    }
}

pub fn get_node<H: Hasher>(data: &[u8], index: usize) -> Result<H::Domain> {
    H::Domain::try_from_bytes(data_at_node(data, index).expect("invalid node math"))
}
