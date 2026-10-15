use cutroom_compose::Canvas;
use std::time::Instant;

#[test]
#[ignore]
fn canvas_new_cost() {
    let start = Instant::now();
    let runs = 300;
    for _ in 0..runs {
        let c = Canvas::new(1080, 1920);
        std::hint::black_box(&c.rgba);
    }
    eprintln!(
        "BENCH Canvas::new: {:.2} ms",
        start.elapsed().as_secs_f64() * 1000.0 / runs as f64
    );
}
