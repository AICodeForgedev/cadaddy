// CADaddy Walk: lit triangles and an edge overlay, in feet (x east, y north, z up).
struct U {
    vp: mat4x4<f32>,
    light: vec4<f32>,
    eye: vec4<f32>,
    // x: cut height (fragments above it are dropped), y: fog distance
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: U;

struct VI {
    @location(0) p: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) c: vec4<f32>,
};
struct VO {
    @builtin(position) pos: vec4<f32>,
    @location(0) c: vec4<f32>,
    @location(1) n: vec3<f32>,
    @location(2) w: vec3<f32>,
};

fn lin(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

fn shade_vertex(v: VI) -> VO {
    var o: VO;
    o.pos = u.vp * vec4<f32>(v.p, 1.0);
    o.c = vec4<f32>(lin(v.c.rgb), v.c.a);
    o.n = v.n;
    o.w = v.p;
    return o;
}

@vertex
fn vs_tri(v: VI) -> VO {
    return shade_vertex(v);
}

// Edges sit a hair in front of the faces they outline.
@vertex
fn vs_line(v: VI) -> VO {
    var o = shade_vertex(v);
    o.pos.z = o.pos.z - 0.00015 * o.pos.w;
    return o;
}

fn fogged(col: vec3<f32>, w: vec3<f32>) -> vec3<f32> {
    let f = clamp(length(w - u.eye.xyz) / u.params.y, 0.0, 1.0);
    return mix(col, lin(vec3<f32>(0.82, 0.85, 0.89)), f * 0.65);
}

@fragment
fn fs_tri(o: VO) -> @location(0) vec4<f32> {
    if (o.w.z > u.params.x) {
        discard;
    }
    let n = normalize(o.n);
    // Ceilings face down and are one-sided: seen from above (cut away, flying) they vanish.
    if (n.z < -0.9 && u.eye.z > o.w.z) {
        discard;
    }
    let d = abs(dot(n, normalize(u.light.xyz)));
    let col = o.c.rgb * (0.38 + 0.62 * d);
    return vec4<f32>(fogged(col, o.w), 1.0);
}

@fragment
fn fs_line(o: VO) -> @location(0) vec4<f32> {
    if (o.w.z > u.params.x) {
        discard;
    }
    return vec4<f32>(fogged(o.c.rgb, o.w), 1.0);
}
