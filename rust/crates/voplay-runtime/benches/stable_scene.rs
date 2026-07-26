use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use voplay_protocol::Handle;
use voplay_runtime::presentation::RenderFrameDelta;
use voplay_runtime::render_world::{
    encode_render_object, Aabb3, RenderWorld, RenderWorldConfig, RetainedRenderObject,
    TransformSample,
};
use voplay_runtime::RenderEntity;

const OBJECT_COUNT: usize = 10_000;
const WARMUP: usize = 20;
const DEFAULT_RUNS: usize = 200;
const MAX_RUNS: usize = 10_000_000;

fn main() {
    let runs = configured_runs("VOPLAY_STABLE_RUNS");
    let engine = Handle {
        index: 0,
        generation: 1,
    };
    let mut world =
        RenderWorld::new(engine, 1, RenderWorldConfig::default()).expect("valid benchmark config");
    let objects = (0..OBJECT_COUNT)
        .map(|index| {
            let entity = entity(engine, index);
            (
                entity,
                encode_render_object(&object(entity, 0), RenderWorldConfig::default())
                    .expect("encode stable object"),
            )
        })
        .collect();
    world
        .apply_delta(&RenderFrameDelta {
            revision: 1,
            full_snapshot: true,
            objects,
            despawned: Vec::new(),
        })
        .expect("initial stable scene fixture");
    world
        .poll_dirty_uploads(OBJECT_COUNT)
        .expect("initial upload");

    for iteration in 0..WARMUP {
        update_one(&mut world, engine, iteration);
    }
    let before = world.instrumentation();
    let mut samples = Vec::with_capacity(runs);
    for iteration in 0..runs {
        let start = Instant::now();
        update_one(&mut world, engine, WARMUP + iteration);
        samples.push(start.elapsed().as_nanos());
    }
    let counters = world.instrumentation();
    assert_eq!(
        counters.last_delta_objects, 1,
        "stable scene benchmark processed unrelated objects"
    );
    assert_eq!(
        counters.decoded_objects - before.decoded_objects,
        runs as u64,
        "stable scene benchmark decoded unrelated objects"
    );
    assert_eq!(
        counters.queued_dirty_uploads - before.queued_dirty_uploads,
        runs as u64,
        "stable scene benchmark queued unrelated uploads"
    );
    print_report(
        "voplay-stable-scene-single-object",
        runs,
        &mut samples,
        counters,
    );
}

fn update_one(world: &mut RenderWorld, engine: Handle, iteration: usize) {
    let selected = iteration % OBJECT_COUNT;
    let entity = entity(engine, selected);
    let revision = world.revision() + 1;
    world
        .apply_delta(&RenderFrameDelta {
            revision,
            full_snapshot: false,
            objects: vec![(
                entity,
                encode_render_object(&object(entity, revision), RenderWorldConfig::default())
                    .expect("encode changed object"),
            )],
            despawned: Vec::new(),
        })
        .expect("single object delta");
    black_box(world.poll_dirty_uploads(usize::MAX).expect("dirty upload"));
}

fn entity(engine: Handle, index: usize) -> RenderEntity {
    RenderEntity {
        engine,
        entity: Handle {
            index: index as u32,
            generation: 1,
        },
    }
}

fn object(entity: RenderEntity, revision: u64) -> RetainedRenderObject {
    let x = entity.entity.index as i64 * 2_000;
    RetainedRenderObject {
        entity,
        transform: TransformSample {
            previous_tick: revision.saturating_sub(1),
            current_tick: revision,
            previous: [x; 12],
            current: [x.saturating_add(revision as i64); 12],
        },
        bounds: Aabb3 {
            min: [x, 0, 0],
            max: [x + 100, 100, 100],
        },
        layers: 1,
        components: BTreeMap::new(),
    }
}

fn print_report(
    name: &str,
    runs: usize,
    samples: &mut [u128],
    counters: voplay_runtime::render_world::RenderWorldInstrumentation,
) {
    samples.sort_unstable();
    println!(
        concat!(
            "{{\"name\":\"{}\",\"fixture_objects\":{},\"warmup\":{},\"runs\":{},",
            "\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{},",
            "\"decoded_objects\":{},\"queued_dirty_uploads\":{},\"objects\":{}}}"
        ),
        name,
        OBJECT_COUNT,
        WARMUP,
        runs,
        percentile(samples, 50),
        percentile(samples, 95),
        percentile(samples, 99),
        samples[samples.len() - 1],
        counters.decoded_objects,
        counters.queued_dirty_uploads,
        counters.objects,
    );
}

fn configured_runs(name: &str) -> usize {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|runs| (1..=MAX_RUNS).contains(runs))
            .unwrap_or_else(|| panic!("{name} must be an integer in 1..={MAX_RUNS}")),
        Err(std::env::VarError::NotPresent) => DEFAULT_RUNS,
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} must be valid UTF-8"),
    }
}

fn percentile(samples: &[u128], percentile: usize) -> u128 {
    let index = (samples.len() - 1) * percentile / 100;
    samples[index]
}
