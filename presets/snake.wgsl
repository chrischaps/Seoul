// Snake — a vertical wiggling line drawn in chunky pixelated cells with
// hash-based dithering, so the line looks like a scattering of bright dots
// whose density falls off perpendicular to the path. The feedback warp is
// disabled in the TOML mapping (zoom=1, warp=0) so dots stay where they
// were drawn; decay alone fades the trail over ~1.5 seconds.
//
// Two superimposed sines drive the wiggle so the motion never repeats
// cleanly — feels organic rather than robotic.

fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment
fn fs_composite(in: Varying) -> @location(0) vec4<f32> {
    let t = u.time;

    // Pixelate into square cells (100 rows) in aspect-corrected space,
    // then map each cell center back to UV for the path math.
    let rows = 100.0;
    let cell = floor(vec2<f32>(in.uv.x * u.aspect, in.uv.y) * rows);
    let pc = (cell + 0.5) / rows;
    let p = vec2<f32>(pc.x / u.aspect, pc.y);

    // Wiggling line: two sines with different freqs/phases mix into an
    // organic-looking path. Bass swells the lateral amplitude.
    let amp = 0.14 + u.bass * 0.22;
    let wiggle = sin(p.y * 7.5 + t * 0.55) * 0.65
               + sin(p.y * 14.2 + t * 0.41 + 1.7) * 0.35;
    let line_x = 0.5 + amp * wiggle;

    // Fat blob with sharp boundaries. Interior gets uniform-density blue
    // dither (every cell of the same parity is lit, regardless of position
    // in the blob — gives a flat fill, not a Gaussian fade). Perimeter gets
    // a pink-white outline.
    let blob_half = 0.040 + u.bass * 0.020 + u.beat * 0.018;
    let edge_w = 0.005;
    let d = abs(p.x - line_x);

    let in_blob = step(d, blob_half);
    let on_edge = in_blob * step(blob_half - edge_w, d);
    let interior = in_blob * (1.0 - on_edge);

    // Uniform 50% checkerboard fill for the blob interior — looks like
    // a regularly-stippled solid color, not a probability gradient.
    let checker = f32((i32(cell.x) + i32(cell.y)) & 1);
    let blue_lit = interior * checker;

    let blue_contrib = palette.colors[1].rgb * blue_lit * 0.040;

    // Pink-white outline at the perimeter, full strength (no dither).
    let pinkish = mix(palette.colors[3].rgb, vec3<f32>(1.0, 0.86, 0.93), 0.35);
    let edge_contrib = pinkish * on_edge * (0.08 + u.beat * 0.06);

    return vec4<f32>(blue_contrib + edge_contrib, 1.0);
}
