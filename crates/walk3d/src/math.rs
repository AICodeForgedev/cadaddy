//! Small 3D vector and 4×4 matrix helpers (row-major; [`Mat4::to_gpu`] gives WGSL's column order).

use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub const fn v3(x: f64, y: f64, z: f64) -> V3 {
    V3 { x, y, z }
}

impl V3 {
    pub fn dot(self, o: V3) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: V3) -> V3 {
        v3(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }
    pub fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    pub fn norm(self) -> V3 {
        let l = self.len();
        if l > 1e-12 { self * (1.0 / l) } else { self }
    }
    pub fn f32(self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }
}

impl Add for V3 {
    type Output = V3;
    fn add(self, o: V3) -> V3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for V3 {
    type Output = V3;
    fn sub(self, o: V3) -> V3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f64> for V3 {
    type Output = V3;
    fn mul(self, s: f64) -> V3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
}

/// Row-major 4×4 matrix acting on column vectors (`m * p`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [[f64; 4]; 4]);

impl Mat4 {
    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut r = [[0.0; 4]; 4];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.0[i][k] * o.0[k][j]).sum();
            }
        }
        Mat4(r)
    }
    /// Camera at `eye` looking along `fwd` with z up (view space looks down −z).
    pub fn look(eye: V3, fwd: V3) -> Mat4 {
        let f = fwd.norm();
        let mut r = f.cross(v3(0.0, 0.0, 1.0));
        if r.len() < 1e-9 {
            r = v3(1.0, 0.0, 0.0);
        }
        let r = r.norm();
        let u = r.cross(f);
        Mat4([[r.x, r.y, r.z, -r.dot(eye)], [u.x, u.y, u.z, -u.dot(eye)], [-f.x, -f.y, -f.z, f.dot(eye)], [0.0, 0.0, 0.0, 1.0]])
    }
    /// Right-handed perspective with depth 0..1 (wgpu clip space).
    pub fn perspective(fovy: f64, aspect: f64, near: f64, far: f64) -> Mat4 {
        let f = 1.0 / (fovy / 2.0).tan();
        Mat4([[f / aspect, 0.0, 0.0, 0.0], [0.0, f, 0.0, 0.0], [0.0, 0.0, far / (near - far), near * far / (near - far)], [0.0, 0.0, -1.0, 0.0]])
    }
    /// Column-major f32 for a WGSL `mat4x4<f32>`.
    pub fn to_gpu(&self) -> [[f32; 4]; 4] {
        let mut c = [[0.0f32; 4]; 4];
        for (i, row) in self.0.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                c[j][i] = *v as f32;
            }
        }
        c
    }
}
