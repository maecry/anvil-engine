// Equirectangular mapping shared by the environment bakes and the forward shader.
const float PI = 3.14159265;

// u wraps around +Y, v = 0 at +Y (top), v = 1 at -Y.
vec2 dir_to_uv(vec3 d) {
    return vec2(atan(d.z, d.x) / (2.0 * PI) + 0.5, acos(clamp(d.y, -1.0, 1.0)) / PI);
}

vec3 uv_to_dir(vec2 uv) {
    float phi = (uv.x - 0.5) * 2.0 * PI;
    float theta = uv.y * PI;
    return vec3(sin(theta) * cos(phi), cos(theta), sin(theta) * sin(phi));
}
