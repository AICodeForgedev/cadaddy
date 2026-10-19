// CADaddy shaded model view: lit triangles and lines, orthographic, positions relative to an
// origin. The Realistic style lights physically (cadaddy_render::light, the same equations): sky
// and sun (with a shadow map), up to 16 lights, microfacet materials with base colour textures,
// a filmic tone curve with exposure.
struct Light {
    // xyz: position (distant: towards the light); w: kind (0 distant, 1 point, 2 spot)
    pos: vec4<f32>,
    // xyz: spot direction; w: cos of the hotspot half-angle
    dir: vec4<f32>,
    // rgb: colour × intensity; w: cos of the falloff half-angle
    rgb: vec4<f32>,
    // x: attenuation (0 none, 1 1/d, 2 1/d²)
    att: vec4<f32>,
};

struct U {
    vp: mat4x4<f32>,
    // xyz: towards the light
    light: vec4<f32>,
    // xyz: towards the viewer
    eye: vec4<f32>,
    // rgb: background (Hidden fills faces with it); a: 1 for highlights (Realistic)
    bg: vec4<f32>,
    // x: 1 when faces take the background colour (Hidden); y: 1 for physical lighting;
    // z: exposure (2^stops); w: number of lights
    params: vec4<f32>,
    // The scene's origin (positions are relative to it), w: metres per drawing unit
    origin: vec4<f32>,
    // xyz: towards the sun; w: 1 when the sun shines
    sun_dir: vec4<f32>,
    sun_rgb: vec4<f32>,
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    ground: vec4<f32>,
    // World (relative) → shadow map clip space
    shadow_vp: mat4x4<f32>,
    // x: shadow depth bias (depth units); y: 1 when there is a shadow map; z: normal offset
    // (world units)
    shadow: vec4<f32>,
    lights: array<Light, 16>,
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex: texture_2d_array<f32>;
@group(0) @binding(2) var tex_sampler: sampler;
@group(0) @binding(3) var shadow_map: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
// Baked indirect light (LIGHTBAKE), RGBM: colour × alpha × 8.
@group(0) @binding(5) var lm_tex: texture_2d<f32>;
@group(0) @binding(6) var lm_sampler: sampler;

struct VI {
    @location(0) p: vec3<f32>,
    @location(1) n: vec3<f32>,
    @location(2) c: vec4<f32>,
    @location(3) uv: vec2<f32>,
    // roughness, metallic, texture layer (−1: none), opacity
    @location(4) m: vec4<f32>,
    // lightmap coordinates of the front (xy) and back (zw); negative: none
    @location(8) lm: vec4<f32>,
};
// An instance's placement: the rows of a 3 × 4 matrix (block coordinates to the view's, relative
// to its origin). The base geometry draws with the identity.
struct II {
    @location(5) r0: vec4<f32>,
    @location(6) r1: vec4<f32>,
    @location(7) r2: vec4<f32>,
};

fn place(i: II, p: vec3<f32>) -> vec3<f32> {
    let q = vec4<f32>(p, 1.0);
    return vec3<f32>(dot(i.r0, q), dot(i.r1, q), dot(i.r2, q));
}

fn turn(i: II, n: vec3<f32>) -> vec3<f32> {
    let t = vec3<f32>(dot(i.r0.xyz, n), dot(i.r1.xyz, n), dot(i.r2.xyz, n));
    let l = length(t);
    return select(n, t / l, l > 1e-12);
}
struct VO {
    @builtin(position) pos: vec4<f32>,
    @location(0) c: vec4<f32>,
    @location(1) n: vec3<f32>,
    @location(2) wp: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) m: vec4<f32>,
    @location(5) lm: vec4<f32>,
};

const PI: f32 = 3.14159265;

fn lin(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

// The target holds display (sRGB-encoded) values, as egui shows them.
fn enc(c: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(pow(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / 2.2)), 1.0);
}

// The ACES fitted tone curve, exposure applied, then the sRGB curve.
fn tonemap(c: vec3<f32>) -> vec3<f32> {
    let x = max(c * u.params.z * 0.8, vec3<f32>(0.0));
    let y = clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = y * 12.92;
    let hi = 1.055 * pow(y, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, y <= vec3<f32>(0.0031308));
}

@vertex
fn vs_tri(v: VI, i: II) -> VO {
    var o: VO;
    let p = place(i, v.p);
    o.pos = u.vp * vec4<f32>(p, 1.0);
    o.c = vec4<f32>(lin(v.c.rgb), v.c.a);
    o.n = turn(i, v.n);
    o.wp = p;
    o.uv = v.uv;
    o.m = v.m;
    o.lm = v.lm;
    return o;
}

// Lines sit a little in front of the faces they lie on.
@vertex
fn vs_line(v: VI, i: II) -> VO {
    var o: VO;
    let p = place(i, v.p);
    o.pos = u.vp * vec4<f32>(p, 1.0);
    o.pos.z = o.pos.z - 0.002 * o.pos.w;
    o.c = vec4<f32>(lin(v.c.rgb), v.c.a);
    o.n = turn(i, v.n);
    o.wp = p;
    o.uv = v.uv;
    o.m = v.m;
    o.lm = v.lm;
    return o;
}

// The sun's view, for the shadow map.
@vertex
fn vs_shadow(v: VI, i: II) -> @builtin(position) vec4<f32> {
    return u.shadow_vp * vec4<f32>(place(i, v.p), 1.0);
}

// The sky behind a Realistic view: a full-screen triangle, horizon to zenith.
struct BgO {
    @builtin(position) pos: vec4<f32>,
    @location(0) t: f32,
};
@vertex
fn vs_bg(@builtin(vertex_index) i: u32) -> BgO {
    var o: BgO;
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    o.pos = vec4<f32>(x, y, 1.0, 1.0);
    o.t = y * 0.5 + 0.5;
    return o;
}
@fragment
fn fs_bg(o: BgO) -> @location(0) vec4<f32> {
    let c = mix(u.horizon.rgb, u.zenith.rgb, pow(clamp(o.t, 0.0, 1.0), 0.8));
    return vec4<f32>(tonemap(c), 1.0);
}

fn brdf_cos(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, base: vec3<f32>, rough: f32, metal: f32) -> vec3<f32> {
    let nl = dot(n, l);
    if (nl <= 0.0) {
        return vec3<f32>(0.0);
    }
    let nv = max(dot(n, v), 1e-4);
    let h = normalize(l + v);
    let nh = max(dot(n, h), 0.0);
    let vh = max(dot(v, h), 0.0);
    let a = clamp(rough, 0.04, 1.0) * clamp(rough, 0.04, 1.0);
    let a2 = a * a;
    let dd = nh * nh * (a2 - 1.0) + 1.0;
    let d = a2 / max(PI * dd * dd, 1e-8);
    let vis = 0.5 / max(nl * sqrt(nv * nv * (1.0 - a2) + a2) + nv * sqrt(nl * nl * (1.0 - a2) + a2), 1e-8);
    let f0 = mix(vec3<f32>(0.04), base, metal);
    let f = f0 + (vec3<f32>(1.0) - f0) * pow(1.0 - vh, 5.0);
    let diffuse = (vec3<f32>(1.0) - f) * (1.0 - metal) * base / PI;
    return (diffuse + vec3<f32>(d * vis) * f) * nl;
}

fn ambient(n: vec3<f32>, base: vec3<f32>, rough: f32, metal: f32) -> vec3<f32> {
    let up = clamp(n.z, -1.0, 1.0);
    let sky = mix(u.horizon.rgb, u.zenith.rgb, max(up, 0.0));
    var env = mix(u.ground.rgb, sky, up * 0.5 + 0.5);
    let grey = (env.r + env.g + env.b) / 3.0;
    env = mix(env, vec3<f32>(grey), 0.5);
    return surround(env, base, rough, metal);
}

// What light `env` arriving from all around does on a surface (cadaddy_render::light::surround).
fn surround(env: vec3<f32>, base: vec3<f32>, rough: f32, metal: f32) -> vec3<f32> {
    let kd = (1.0 - metal) * 0.96;
    let spec = mix(vec3<f32>(0.04), base, metal);
    let gloss = clamp(1.0 - rough, 0.0, 1.0) * 0.5;
    return env * (base * kd + spec * (0.3 + gloss));
}

// How lit by the sun the point is (0 shadowed … 1 lit), 3 × 3 samples.
fn sun_visibility(wp: vec3<f32>, n: vec3<f32>) -> f32 {
    if (u.shadow.y < 0.5) {
        return 1.0;
    }
    let c = u.shadow_vp * vec4<f32>(wp + n * u.shadow.z, 1.0);
    let uv = vec2<f32>(c.x * 0.5 + 0.5, 0.5 - c.y * 0.5);
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return 1.0;
    }
    let texel = 1.0 / f32(textureDimensions(shadow_map).x);
    var sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_sampler, uv + vec2<f32>(f32(x), f32(y)) * texel, c.z - u.shadow.x);
        }
    }
    return sum / 9.0;
}

fn physical(o: VO, n: vec3<f32>, back: bool) -> vec4<f32> {
    var base = o.c.rgb;
    if (o.m.z >= 0.0) {
        base = base * textureSample(tex, tex_sampler, vec2<f32>(o.uv.x, 1.0 - o.uv.y), i32(o.m.z + 0.5)).rgb;
    }
    let rough = o.m.x;
    let metal = o.m.y;
    let v = normalize(u.eye.xyz);
    var col = ambient(n, base, rough, metal);
    if (o.lm.x >= 0.0) {
        let t = textureSampleLevel(lm_tex, lm_sampler, select(o.lm.xy, o.lm.zw, back), 0.0);
        col = surround(t.rgb * t.a * 8.0, base, rough, metal);
    }
    if (u.sun_dir.w > 0.5) {
        col = col + u.sun_rgb.rgb * sun_visibility(o.wp, n) * brdf_cos(n, v, normalize(u.sun_dir.xyz), base, rough, metal);
    }
    let count = i32(u.params.w);
    for (var i = 0; i < 16; i = i + 1) {
        if (i >= count) {
            break;
        }
        let l = u.lights[i];
        if (l.pos.w < 0.5) {
            col = col + l.rgb.rgb * brdf_cos(n, v, normalize(l.pos.xyz), base, rough, metal);
            continue;
        }
        let to = l.pos.xyz - o.wp;
        let dist = length(to);
        if (dist < 1e-6) {
            continue;
        }
        let ld = to / dist;
        let dm = max(dist * u.origin.w, 0.05);
        var fall = 1.0;
        if (l.att.x > 1.5) {
            fall = 1.0 / (dm * dm);
        } else if (l.att.x > 0.5) {
            fall = 1.0 / dm;
        }
        var cone = 1.0;
        if (l.pos.w > 1.5) {
            cone = clamp((dot(-ld, l.dir.xyz) - l.rgb.w) / max(l.dir.w - l.rgb.w, 1e-4), 0.0, 1.0);
        }
        col = col + l.rgb.rgb * (fall * cone) * brdf_cos(n, v, ld, base, rough, metal);
    }
    return vec4<f32>(tonemap(col), o.m.w);
}

@fragment
fn fs_tri(o: VO) -> @location(0) vec4<f32> {
    if (u.params.x > 0.5) {
        return vec4<f32>(u.bg.rgb, 1.0);
    }
    var n = normalize(o.n);
    // Seen from its back (the corners turn clockwise from the eye).
    let back = dot(n, u.eye.xyz) < 0.0;
    if (back) {
        n = -n;
    }
    if (u.params.y > 0.5) {
        return physical(o, n, back);
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
