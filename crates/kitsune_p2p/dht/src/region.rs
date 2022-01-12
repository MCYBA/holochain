mod region_coords;
mod region_data;
mod region_set;

pub use region_coords::*;
pub use region_data::*;
pub use region_set::*;

use crate::{coords::*, tree::*};

#[derive(Debug, derive_more::Constructor)]
pub struct RegionImpl<T: TreeDataConstraints> {
    pub coords: RegionCoords,
    pub data: T,
}

impl<T: TreeDataConstraints> RegionImpl<T> {
    pub const MASS: u32 = std::mem::size_of::<Region>() as u32;

    pub fn split(self, tree: &TreeImpl<T>) -> Option<(Self, Self)> {
        let (c1, c2) = self.coords.halve()?;
        let d1 = tree.lookup(&c1.to_bounds());
        let d2 = tree.lookup(&c2.to_bounds());
        let r1 = Self {
            coords: c1,
            data: d1,
        };
        let r2 = Self {
            coords: c2,
            data: d2,
        };
        Some((r1, r2))
    }
}

pub type Region = RegionImpl<RegionData>;