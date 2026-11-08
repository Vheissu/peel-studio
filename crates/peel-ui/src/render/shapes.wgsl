// Instanced signed-distance shapes. One instance per shape; every shape is
// drawn as a 4-vertex strip covering its bounds plus an anti-aliasing margin.
//
// Kinds: 0 rounded rect (fill, border, vertical gradient, blur/shadow)
//        1 arc / ring with round caps
//        2 line segment with round caps
//        3 triangle filling its rect, pointing right/down/left/up
//
// Colors arrive in sRGB with straight alpha and are written premultiplied;
// the surface is non-sRGB so blending happens in sRGB space like a browser.

struct Globals {
    viewport: vec2<f32>,
    scale: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) rect: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) color2: vec4<f32>,
    @location(3) radii: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) extra: vec4<f32>,
    @location(6) clip: vec4<f32>,
};

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) p: vec2<f32>,
    @location(1) @interpolate(flat) rect: vec4<f32>,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) color2: vec4<f32>,
    @location(4) @interpolate(flat) radii: vec4<f32>,
    @location(5) @interpolate(flat) params: vec4<f32>,
    @location(6) @interpolate(flat) extra: vec4<f32>,
    @location(7) @interpolate(flat) clip: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VsOut {
    let s = globals.scale;
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u));
    let kind = inst.params.x;
    let blur = inst.params.z * s;
    let pad = 1.5 + blur;

    let r = inst.rect * s;
    let pos = r.xy - vec2<f32>(pad) + corner * (r.zw + vec2<f32>(2.0 * pad));

    var out: VsOut;
    let ndc = pos / globals.viewport * 2.0 - 1.0;
    out.position = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.p = pos;
    out.rect = r;
    out.color = inst.color;
    out.color2 = inst.color2;
    out.radii = inst.radii * s;
    out.params = vec4<f32>(kind, inst.params.y * s, blur, inst.params.w);
    if (kind == 1.0) {
        // arc: radius, thickness scale; angles do not
        out.extra = vec4<f32>(inst.extra.x * s, inst.extra.y * s, inst.extra.z, inst.extra.w);
    } else if (kind == 2.0) {
        out.extra = inst.extra * s;
    } else {
        out.extra = inst.extra;
    }
    out.clip = inst.clip * s;
    return out;
}

fn sd_round_box(p: vec2<f32>, half: vec2<f32>, radii: vec4<f32>) -> f32 {
    // radii: top-left, top-right, bottom-right, bottom-left (y down)
    var r: f32;
    if (p.x < 0.0) {
        r = select(radii.w, radii.x, p.y < 0.0);
    } else {
        r = select(radii.z, radii.y, p.y < 0.0);
    }
    r = min(r, min(half.x, half.y));
    let q = abs(p) - half + vec2<f32>(r);
    return min(max(q.x, q.y), 0.0) + length(max(q, vec2<f32>(0.0))) - r;
}

fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    return length(pa - ba * h);
}

fn sd_triangle(p: vec2<f32>, p0: vec2<f32>, p1: vec2<f32>, p2: vec2<f32>) -> f32 {
    let e0 = p1 - p0;
    let e1 = p2 - p1;
    let e2 = p0 - p2;
    let v0 = p - p0;
    let v1 = p - p1;
    let v2 = p - p2;
    let pq0 = v0 - e0 * clamp(dot(v0, e0) / dot(e0, e0), 0.0, 1.0);
    let pq1 = v1 - e1 * clamp(dot(v1, e1) / dot(e1, e1), 0.0, 1.0);
    let pq2 = v2 - e2 * clamp(dot(v2, e2) / dot(e2, e2), 0.0, 1.0);
    let sgn = sign(e0.x * e2.y - e0.y * e2.x);
    let d = min(min(vec2<f32>(dot(pq0, pq0), sgn * (v0.x * e0.y - v0.y * e0.x)),
                    vec2<f32>(dot(pq1, pq1), sgn * (v1.x * e1.y - v1.y * e1.x))),
                    vec2<f32>(dot(pq2, pq2), sgn * (v2.x * e2.y - v2.y * e2.x)));
    return -sqrt(d.x) * sign(d.y);
}

const TAU: f32 = 6.28318530718;

fn sd_arc(p: vec2<f32>, radius: f32, thickness: f32, start: f32, sweep: f32) -> f32 {
    let ring = abs(length(p) - radius) - thickness * 0.5;
    if (sweep >= TAU - 1e-4) {
        return ring;
    }
    // Angle measured from 12 o'clock, clockwise (y points down).
    let ang = atan2(p.x, -p.y);
    var rel = ang - start;
    rel = rel - floor(rel / TAU) * TAU;
    if (rel <= sweep) {
        return ring;
    }
    let e0 = radius * vec2<f32>(sin(start), -cos(start));
    let e1 = radius * vec2<f32>(sin(start + sweep), -cos(start + sweep));
    return min(length(p - e0), length(p - e1)) - thickness * 0.5;
}

fn coverage(d: f32, blur: f32) -> f32 {
    if (blur > 0.0) {
        return 1.0 - smoothstep(-blur, blur, d);
    }
    return clamp(0.5 - d, 0.0, 1.0);
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = in.p;
    if (p.x < in.clip.x || p.y < in.clip.y || p.x >= in.clip.z || p.y >= in.clip.w) {
        discard;
    }
    let kind = in.params.x;
    let stroke = in.params.y;
    let blur = in.params.z;
    let center = in.rect.xy + in.rect.zw * 0.5;
    let local = p - center;

    if (kind == 0.0) {
        let d = sd_round_box(local, in.rect.zw * 0.5, in.radii);
        var fill = in.color;
        if (in.params.w == 1.0) {
            let t = clamp((p.y - in.rect.y) / max(in.rect.w, 1.0), 0.0, 1.0);
            fill = mix(in.color, in.extra, t);
        } else if (in.params.w == 2.0) {
            let t = clamp((p.x - in.rect.x) / max(in.rect.z, 1.0), 0.0, 1.0);
            fill = mix(in.color, in.extra, t);
        }
        let outer = coverage(d, blur);
        if (stroke > 0.0) {
            let inner = clamp(0.5 - (d + stroke), 0.0, 1.0);
            let c = premul(fill) * inner + premul(in.color2) * (1.0 - inner);
            return c * outer;
        }
        return premul(fill) * outer;
    } else if (kind == 1.0) {
        let d = sd_arc(local, in.extra.x, in.extra.y, in.extra.z, in.extra.w);
        return premul(in.color) * coverage(d, blur);
    } else if (kind == 2.0) {
        let d = sd_segment(p, in.extra.xy, in.extra.zw) - stroke * 0.5;
        return premul(in.color) * coverage(d, blur);
    } else {
        let r = in.rect;
        var a: vec2<f32>;
        var b: vec2<f32>;
        var c: vec2<f32>;
        let dir = in.params.w;
        if (dir == 0.0) {
            a = r.xy; b = vec2<f32>(r.x + r.z, r.y + r.w * 0.5); c = vec2<f32>(r.x, r.y + r.w);
        } else if (dir == 1.0) {
            a = r.xy; b = vec2<f32>(r.x + r.z, r.y); c = vec2<f32>(r.x + r.z * 0.5, r.y + r.w);
        } else if (dir == 2.0) {
            a = vec2<f32>(r.x + r.z, r.y); b = vec2<f32>(r.x + r.z, r.y + r.w); c = vec2<f32>(r.x, r.y + r.w * 0.5);
        } else {
            a = vec2<f32>(r.x, r.y + r.w); b = vec2<f32>(r.x + r.z * 0.5, r.y); c = vec2<f32>(r.x + r.z, r.y + r.w);
        }
        let d = sd_triangle(p, a, b, c) - in.radii.x;
        return premul(in.color) * coverage(d, blur);
    }
}
