// Mush8 - motion.rs
// Constantin Clerc (c22dev) - 0.1
// Six-axis fusion ported from the yuzu Emulator Project's MotionInput (GPL-2.0-or-later,
// citron src/hid_core/frontend/motion_input.cpp): a Mahony filter that corrects the gyro with gravity.
// Units are the game's: acceleration in g, gyro in turns per second, time in seconds.

const KP: f32 = 0.3;
const KI: f32 = 0.005;
const GYRO_THRESHOLD: f32 = 0.007;
const AT_REST: f32 = 0.05;
const GYRO_MAX: f32 = 5.0;
const ACCEL_MAX: f32 = 7.0;

type V3 = [f32; 3];

fn len(v: V3) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[derive(Clone, Copy)]
pub struct Fusion {
    quat: [f32; 4],
    accel: V3,
    gyro: V3,
    gyro_bias: V3,
    real_error: V3,
    integral_error: V3,
    rotation: V3,
    only_accel: bool,
}

impl Default for Fusion {
    fn default() -> Self {
        Fusion {
            quat: [0.0, 0.0, 0.0, -1.0],
            accel: [0.0; 3],
            gyro: [0.0; 3],
            gyro_bias: [0.0; 3],
            real_error: [0.0; 3],
            integral_error: [0.0; 3],
            rotation: [0.0; 3],
            only_accel: true,
        }
    }
}

impl Fusion {
    pub fn update(&mut self, accel: V3, gyro: V3, dt: f32) {
        self.accel = accel.map(|a| a.clamp(-ACCEL_MAX, ACCEL_MAX));
        self.gyro = std::array::from_fn(|i| (gyro[i] - self.gyro_bias[i]).clamp(-GYRO_MAX, GYRO_MAX));
        if !self.moving(AT_REST) {
            self.gyro_bias = std::array::from_fn(|i| self.gyro_bias[i] * 0.9999 + gyro[i] * 0.0001);
        }
        if len(self.gyro) < GYRO_THRESHOLD {
            self.gyro = [0.0; 3];
        } else {
            self.only_accel = false;
        }
        self.rotate(dt);
        self.orient(dt);
    }

    fn moving(&self, sensitivity: f32) -> bool {
        let a = len(self.accel);
        len(self.gyro) >= sensitivity || a <= 0.9 || a >= 1.1
    }

    fn rotate(&mut self, dt: f32) {
        if dt <= 0.1 {
            self.rotation = std::array::from_fn(|i| self.rotation[i] + self.gyro[i] * dt);
        }
    }

    fn orient(&mut self, dt: f32) {
        if dt > 0.1 {
            return;
        }
        let [mut q1, q2, q3, q4] = self.quat;
        let tau = std::f32::consts::TAU;
        let mut w = if self.only_accel { [0.0; 3] } else { [self.gyro[1] * tau, -self.gyro[0] * tau, -self.gyro[2] * tau] };
        let a = len(self.accel);
        if (0.75..=1.25).contains(&a) {
            let n = self.accel.map(|v| v / a);
            let (ax, ay, az) = (-n[0], n[1], -n[2]);
            let vx = 2.0 * (q2 * q4 - q1 * q3);
            let vy = 2.0 * (q1 * q2 + q3 * q4);
            let vz = q1 * q1 - q2 * q2 - q3 * q3 + q4 * q4;
            self.real_error = [az * vx - ax * vz, ay * vz - az * vy, ax * vy - ay * vx];
            if len(self.real_error) >= 0.05 {
                self.integral_error = std::array::from_fn(|i| self.integral_error[i] + self.real_error[i]);
            } else {
                self.integral_error = [0.0; 3];
            }
            let (kp, ki) = if self.only_accel { (35.0 * KP, 10.0 * KI) } else { (KP, KI) };
            for i in 0..3 {
                w[i] += kp * self.real_error[i] + ki * self.integral_error[i];
            }
            if self.only_accel {
                self.gyro = [-w[1], w[0], -w[2]];
                self.rotate(dt);
            }
        }
        let (gx, gy, gz) = (w[1], w[0], w[2]);
        let h = 0.5 * dt;
        q1 += (-q2 * gx - q3 * gy - q4 * gz) * h;
        let q = [q1, q2 + (q1 * gx + q3 * gz - q4 * gy) * h, q3 + (q1 * gy - q2 * gz + q4 * gx) * h, q4 + (q1 * gz + q2 * gy - q3 * gx) * h];
        let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        self.quat = q.map(|v| v / l);
    }

    pub fn gyro(&self) -> V3 {
        self.gyro
    }

    pub fn rotation(&self) -> V3 {
        self.rotation
    }

    pub fn orientation(&self) -> [V3; 3] {
        let [qw, qx, qy, qz] = self.quat;
        let (x, y, z, w) = (-qy, -qx, -qw, -qz);
        let m = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y + w * z), 2.0 * (x * z - w * y)],
            [2.0 * (x * y - w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z + w * x)],
            [2.0 * (x * z + w * y), 2.0 * (y * z - w * x), 1.0 - 2.0 * (x * x + y * y)],
        ];
        [[m[0][0], m[0][1], -m[0][2]], [m[1][0], m[1][1], -m[1][2]], [-m[2][0], -m[2][1], m[2][2]]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // expected values come from the yuzu-family MotionInput (citron src/hid_core/frontend/motion_input.cpp),
    // built and run with the same samples
    fn run(f: &mut Fusion, n: usize, accel: [f32; 3], gyro: [f32; 3]) {
        for _ in 0..n {
            f.update(accel, gyro, 0.008333);
        }
    }

    fn close(a: &[f32], b: &[f32]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn matches_reference_fusion() {
        let mut f = Fusion::default();
        run(&mut f, 240, [0.0, 0.0, -1.0], [0.0; 3]);
        assert!(close(f.orientation().as_flattened(), &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]));

        run(&mut f, 120, [0.0, 0.0, -1.0], [0.25, 0.0, 0.0]);
        assert!(close(f.orientation().as_flattened(), &[1.0, 0.0, 0.0, 0.0, 0.301996, 0.953309, 0.0, -0.953309, 0.301996]));
        assert!(close(&f.rotation(), &[0.24999, 0.0, 0.0]));
        assert!(close(&f.gyro(), &[0.25, 0.0, 0.0]));

        run(&mut f, 1200, [-1.0, 0.0, 0.0], [0.0; 3]);
        let rolled = [-0.014437, 0.004343, 0.999887, 0.716666, 0.697379, 0.007319, -0.697268, 0.716690, -0.013181];
        assert!(close(f.orientation().as_flattened(), &rolled));
        assert!(close(&f.gyro(), &[0.0; 3]));

        run(&mut f, 60, [0.0, 0.6, -0.8], [0.0, -0.1, 0.05]);
        let mixed = [0.095546, 0.300315, 0.949043, 0.685034, 0.671894, -0.281581, -0.722219, 0.677030, -0.141530];
        assert!(close(f.orientation().as_flattened(), &mixed));
        assert!(close(&f.rotation(), &[0.24999, -0.049998, 0.024999]));
        assert!(close(&f.gyro(), &[0.0, -0.1, 0.05]));
    }
}
