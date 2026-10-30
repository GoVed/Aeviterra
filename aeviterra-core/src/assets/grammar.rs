use crate::math::fixed::Q64;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaterialType {
    Granite,
    Basalt,
    Marble,
    OakWood,
    Ironwood,
    BronzeAlloy,
    SteelAlloy,
    DamascusAlloy,
    ObsidianGlass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchitecturalStyle {
    NordicTimber,
    ClassicalStone,
    GothicVault,
    FortifiedCitadel,
    SubterraneanDwarven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeraldicField {
    Argent,
    Or,
    Gules,
    Azure,
    Sable,
    Vert,
    Purpure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeraldicCharge {
    None,
    LionRampant,
    EagleDisplayed,
    DragonPassant,
    CrossPattee,
    OakTree,
    TowerTriple,
}

/// Strongly Typed Semantic Heraldry Descriptor (Max 8 bytes).
/// Replaces arbitrary texture image uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeraldryToken {
    pub primary_field: HeraldicField,
    pub secondary_field: HeraldicField,
    pub charge: HeraldicCharge,
    pub charge_color: HeraldicField,
    pub division_style: u8,
}

/// Strongly Typed Structural Macro-Primitive (Max 48 bytes).
/// Replaces raw 3D vertex/index mesh files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralPrimitive {
    pub style: ArchitecturalStyle,
    pub primary_material: MaterialType,
    pub secondary_material: MaterialType,
    pub length: Q64,
    pub height: Q64,
    pub thickness: Q64,
    pub features: u32, // Bitflags for crenellations, arrow slits, buttresses
}

/// Item Synthesis Grammar Token (Max 32 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquipmentGrammar {
    pub item_class_id: u16,
    pub primary_material: MaterialType,
    pub secondary_material: MaterialType,
    pub balance_point: Q64,
    pub mass_grams: u32,
    pub heraldry: Option<HeraldryToken>,
}

pub const VALID_STRUCTURAL_FEATURES_MASK: u32 = 0b111; // Bitflags: 1=crenellations, 2=arrow slits, 4=buttresses
pub const MAX_SLENDERNESS_RATIO: i64 = 32; // Max height-to-thickness ratio before structural buckling

impl StructuralPrimitive {
    /// Validates that physical structural constraints conform to simulation physics.
    pub fn validate_constraints(&self) -> bool {
        // Enforce physical bounds: no zero/negative or absurdly oversized dimensions
        let min_dim = Q64::from_int(1).unwrap();
        let max_dim = Q64::from_int(64).unwrap();

        if self.length < min_dim || self.length > max_dim {
            return false;
        }
        if self.height < min_dim || self.height > max_dim {
            return false;
        }
        if self.thickness < Q64::HALF || self.thickness > Q64::from_int(8).unwrap() {
            return false;
        }
        // Reject unallocated feature bitflags (prevents covert bitflag smuggling)
        if self.features & !VALID_STRUCTURAL_FEATURES_MASK != 0 {
            return false;
        }
        // Enforce structural slenderness ratio (height <= thickness * 32 AND length <= thickness * 32)
        // to prevent paper-thin 64m buckling walls and unsupported 128:1 horizontal beams (E18 & R2-07)
        let max_supported_span = match self.thickness.checked_mul(Q64::from_int(MAX_SLENDERNESS_RATIO).unwrap()) {
            Ok(h) => h,
            Err(_) => return false,
        };
        if self.height > max_supported_span || self.length > max_supported_span {
            return false;
        }
        true
    }
}

impl EquipmentGrammar {
    /// Validates physical equipment parameters (mass, center of mass / balance point) (F19, T10).
    pub fn validate_constraints(&self) -> bool {
        // Mass must be non-zero and within physical bounds (10g to 50kg)
        if self.mass_grams < 10 || self.mass_grams > 50_000 {
            return false;
        }
        // Balance point must be non-negative and bounded (<= 3 meters)
        let max_balance = match Q64::from_int(3) {
            Ok(v) => v,
            Err(_) => return false,
        };
        if self.balance_point < Q64::ZERO || self.balance_point > max_balance {
            return false;
        }
        true
    }
}
