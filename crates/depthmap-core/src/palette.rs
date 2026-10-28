use crate::Material;

/// Number of palette slots the renderer reserves (ids `0..PALETTE_SIZE`).
pub const PALETTE_SIZE: usize = 32;

pub struct MaterialInfo {
    pub name: &'static str,
    /// sRGB, straight alpha.
    pub rgba: [u8; 4],
}

/// Id `0` is "empty" and is never drawn; ids `1..=10` are bound to the
/// number keys `1..9, 0` in the app.
pub const MATERIALS: [MaterialInfo; 11] = [
    MaterialInfo { name: "Empty", rgba: [0, 0, 0, 0] },
    MaterialInfo { name: "Stone floor", rgba: [150, 146, 140, 255] },
    MaterialInfo { name: "Wood floor", rgba: [168, 124, 80, 255] },
    MaterialInfo { name: "Dirt", rgba: [122, 90, 62, 255] },
    MaterialInfo { name: "Grass", rgba: [106, 152, 80, 255] },
    MaterialInfo { name: "Sand", rgba: [224, 202, 142, 255] },
    MaterialInfo { name: "Shallow water", rgba: [118, 176, 216, 255] },
    MaterialInfo { name: "Deep water", rgba: [52, 104, 170, 255] },
    MaterialInfo { name: "Lava", rgba: [234, 98, 34, 255] },
    MaterialInfo { name: "Wall", rgba: [58, 54, 62, 255] },
    MaterialInfo { name: "Door", rgba: [140, 86, 36, 255] },
];

pub fn material_name(m: Material) -> &'static str {
    MATERIALS.get(m as usize).map_or("?", |i| i.name)
}

/// The palette as the renderer's uniform expects it: normalised sRGB values.
/// Unused slots are transparent.
pub fn palette_f32() -> [[f32; 4]; PALETTE_SIZE] {
    let mut out = [[0.0; 4]; PALETTE_SIZE];
    for (slot, info) in out.iter_mut().zip(MATERIALS.iter()) {
        *slot = info.rgba.map(|c| c as f32 / 255.0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_fits_and_empty_is_transparent() {
        assert!(MATERIALS.len() <= PALETTE_SIZE);
        assert_eq!(palette_f32()[0][3], 0.0);
        assert_eq!(material_name(9), "Wall");
        assert_eq!(material_name(200), "?");
    }
}
