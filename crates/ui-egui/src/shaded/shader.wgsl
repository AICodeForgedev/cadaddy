// CADaddy shaded model view: lit triangles and lines, orthographic, positions relative to an origin.
struct U {
    vp: mat4x4<f32>,
    // xyz: towards the light
    light: vec4<f32>,
    // xyz: towards the viewer
    eye: vec4<f32>,
    // rgb: background (Hidden fills faces with it); a: 1 for highlights (Realistic)
    bg: vec4<f32>,
    // x: 1 when faces take the background colour (Hidden)
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
};

fn lin(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

// The target holds display (sRGB-encoded) values, as egui shows them.
fn enc(c: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)), 1.0);
}

@vertex
fn vs_tri(v: VI) -> VO {
    var o: VO;
    o.pos = u.vp * vec4<f32>(v.p, 1.0);
    o.c = vec4<f32>(lin(v.c.rgb), v.c.a);
    o.n = v.n;
    return o;
}

// Lines sit a little in front of the faces they lie on.
@vertex
fn vs_line(v: VI) -> VO {
    var o: VO;
    o.pos = u.vp * vec4<f32>(v.p, 1.0);
    o.pos.z = o.pos.z - 0.002 * o.pos.w;
    o.c = vec4<f32>(lin(v.c.rgb), v.c.a);
    o.n = v.n;
    return o;
}

@fragment
fn fs_tri(o: VO) -> @location(0) vec4<f32> {
    if (u.params.x > 0.5) {
        return vec4<f32>(u.bg.rgb, 1.0);
    }
    var n = normalize(o.n);
    if (dot(n, u.eye.xyz) < 0.0) {
        n = -n;
    }
    let d = max(dot(n, normalize(u.light.xyz)), 0.0);
    var col = o.c.rgb * (0.3 + 0.7 * d);
    if (u.bg.a > 0.5) {
        let h = normalize(normalize(u.light.xyz) + u.eye.xyz);
        col = col + vec3<f32>(pow(max(dot(n, h), 0.0), 24.0) * 0.35);
    }
    return enc(col);
}

@fragment
fn fs_line(o: VO) -> @location(0) vec4<f32> {
    return enc(o.c.rgb);
}
