use pimax_cam::geom;
use pimax_ctrltrack::p3p;
fn main() {
    let x = [[0.03, -0.01, -0.016], [0.029, 0.001, -0.024], [0.044, 0.019, -0.010]];
    let truth = geom::Pose { r: geom::rodrigues([0.3, -0.5, 0.2]), t: [0.05, -0.1, 0.45] };
    let y = x.map(|p| geom::normalize(truth.apply(p)));
    let t = std::time::Instant::now();
    let mut n = 0;
    for _ in 0..100000 {
        n += p3p::p3p(y, x).len();
    }
    println!("p3p: {:.2} us/call ({n})", t.elapsed().as_secs_f64() * 1e6 / 100000.0);
}
