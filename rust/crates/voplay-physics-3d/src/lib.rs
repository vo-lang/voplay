pub use voplay_runtime::physics::*;

pub const SPACE: PhysicsSpace = PhysicsSpace::ThreeD;
pub const FEATURE_ABI: &str = "voplay.physics3d/1";

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    FEATURE_ABI.as_ptr() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_space_and_abi_select_the_three_dimensional_backend() {
        assert_eq!(SPACE, PhysicsSpace::ThreeD);
        assert_eq!(FEATURE_ABI, "voplay.physics3d/1");
        assert_ne!(voplay_profile_link_anchor(), 0);
    }
}
