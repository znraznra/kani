use cutroom_compose::{Canvas, Frame, Placement};
use std::time::Instant;

fn frame(alpha: u8) -> Frame {
    let (w, h) = (1440usize, 1080usize);
    let mut f = Frame::solid(w as u32, h as u32, [0, 0, 0, 255]);
    for i in 0..w * h {
        let v = (i * 73 % 251) as u8;
        f.rgba[i * 4..i * 4 + 4].copy_from_slice(&[v, v.wrapping_mul(3), 255 - v, alpha]);
    }
    f
}

#[test]
#[ignore]
fn regress() {
    let solid = frame(255);
    let soft = frame(200);
    let cases: Vec<(&str, &Frame, Placement)> = vec![
        (
            "scale 1.0",
            &solid,
            Placement {
                scale: 1.0,
                ..Placement::default()
            },
        ),
        (
            "scale 1.371",
            &solid,
            Placement {
                scale: 1.371,
                ..Placement::default()
            },
        ),
        (
            "scale 2.554",
            &solid,
            Placement {
                scale: 2.554,
                ..Placement::default()
            },
        ),
        (
            "skew 8 @1.371",
            &solid,
            Placement {
                scale: 1.371,
                skew_x: 8.0,
                ..Placement::default()
            },
        ),
        (
            "persp .3 @1.371",
            &solid,
            Placement {
                scale: 1.371,
                persp_x: 0.3,
                ..Placement::default()
            },
        ),
        (
            "persp+skew @1.371",
            &solid,
            Placement {
                scale: 1.371,
                persp_x: 0.2,
                skew_y: 6.0,
                y: 0.05,
                ..Placement::default()
            },
        ),
        (
            "rot 20 @1.0",
            &solid,
            Placement {
                scale: 1.0,
                rotation: 20.0,
                ..Placement::default()
            },
        ),
        (
            "skew+see-through @1.0",
            &soft,
            Placement {
                scale: 1.0,
                skew_x: 8.0,
                opacity: 0.8,
                ..Placement::default()
            },
        ),
        (
            "rot+flip @1.79",
            &solid,
            Placement {
                scale: 1.79,
                rotation: 10.0,
                flip_h: true,
                ..Placement::default()
            },
        ),
    ];
    for (name, f, p) in cases {
        let mut canvas = Canvas::new(1080, 1920);
        canvas.draw(f, &p);
        let start = Instant::now();
        let runs = 15;
        for _ in 0..runs {
            canvas.draw(f, &p);
        }
        eprintln!(
            "BENCH {name}: {:.2} ms",
            start.elapsed().as_secs_f64() * 1000.0 / runs as f64
        );
    }
}
