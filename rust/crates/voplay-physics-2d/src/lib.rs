pub use voplay_runtime::physics::*;

pub const SPACE: PhysicsSpace = PhysicsSpace::TwoD;
pub const FEATURE_ABI: &str = "voplay.physics2d/1";

pub fn validate_body(descriptor: PhysicsBodyDescriptor) -> bool {
    descriptor.pose[2] == 0 && descriptor.velocity[2] == 0
}

pub fn validate_collider(descriptor: PhysicsColliderDescriptor) -> bool {
    descriptor.half_extent[2] == 0
}

#[inline(never)]
pub fn voplay_profile_link_anchor() -> usize {
    module_path!().as_ptr() as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use voplay_protocol::Handle;
    use voplay_runtime::world::{WorldEntity, WorldId};

    fn handle(index: u32) -> Handle {
        Handle {
            index,
            generation: 1,
        }
    }

    fn world() -> WorldId {
        WorldId {
            engine: handle(1),
            handle: handle(2),
        }
    }

    #[test]
    fn two_dimensional_contract_rejects_z_pose_velocity_and_extent() {
        let body = PhysicsBodyDescriptor {
            entity: WorldEntity {
                world: world(),
                entity: handle(3),
            },
            kind: BodyKind::Dynamic,
            pose: [1, 2, 0],
            velocity: [3, 4, 0],
        };
        assert!(validate_body(body));
        assert!(!validate_body(PhysicsBodyDescriptor {
            pose: [1, 2, 1],
            ..body
        }));
        let collider = PhysicsColliderDescriptor {
            body: PhysicsBodyId {
                world: world(),
                handle: handle(4),
            },
            half_extent: [10, 20, 0],
            layer: 1,
            mask: 1,
            sensor: false,
        };
        assert!(validate_collider(collider));
        assert!(!validate_collider(PhysicsColliderDescriptor {
            half_extent: [10, 20, 1],
            ..collider
        }));
        assert_eq!(SPACE, PhysicsSpace::TwoD);
    }
}
