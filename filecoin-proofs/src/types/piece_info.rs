use std::fmt;

use crate::types::{Commitment, UnpaddedBytesAmount};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct PieceInfo {
    pub commitment: Commitment,
    pub size: UnpaddedBytesAmount,
}

impl fmt::Debug for PieceInfo {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("PieceInfo")
            .field("commitment", &hex::encode(&self.commitment))
            .field("size", &self.size)
            .finish()
    }
}

impl PieceInfo {
    pub fn new(commitment: Commitment, size: UnpaddedBytesAmount) -> Self {
        PieceInfo { commitment, size }
    }
}
