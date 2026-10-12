// Full-screen pass: every output pixel casts a ray from the camera through the
// (optionally warped) screen position and lands on the floating desktop plane.
// All parameters arrive packed in c[0..8] (see render.rs, `write_constants`).

cbuffer C : register(b0) { float4 c[10]; };

Texture2D scr : register(t0);   // the captured desktop, with a full mip chain
Texture2D cur : register(t1);   // the mouse cursor, 128x128 straight alpha
SamplerState sAniso : register(s0);
SamplerState sLin : register(s1);

struct VO { float4 pos : SV_Position; };

VO vs(uint id : SV_VertexID)
{
    VO o;
    float2 t = float2((id << 1) & 2, id & 2);
    o.pos = float4(t * float2(2, -2) + float2(-1, 1), 0, 1);
    return o;
}

// Identity up to R, then it stretches more and more: the center stays at full
// magnification and the edges show progressively more of the plane.
// The stretch is a * (1 + A d^2), not a + A d^2: with R = 0 the latter is
// a + A|p| in vector form, a cone whose tip shows as a bump at the center.
// This one is a cubic, smooth through the origin.
float warp1(float a, float R, float A)
{
    float d = max(a - R, 0);
    return a * (1 + A * d * d);
}

// "Sharp bilinear": texels stay square, only the seams between them are
// anti-aliased by one screen pixel. This is the blocky look.
float2 sharpUV(float2 t)
{
    float2 fw = max(fwidth(t), 1e-5);
    float2 s = floor(t + 0.5);
    return s + clamp((t - s) / fw, -0.5, 0.5);
}

float4 ps(VO i) : SV_Target
{
    float2 scrSize = c[0].xy;
    float2 texSize = c[0].zw;
    float3 P = c[1].xyz;
    float th = c[1].w;
    float3 right = c[2].xyz;
    float aspect = c[2].w;
    float3 up = c[3].xyz;
    int filt = (int)c[3].w;
    float3 fwd = c[4].xyz;
    bool warpOn = c[4].w > 0.5;
    float2 M = c[5].xy;
    float curVis = c[5].z;
    float2 hot = c[6].xy;
    int shape = (int)c[6].z;
    float R = c[6].w;
    float A = c[7].x;
    float sT = c[7].y, sB = c[7].z, sL = c[7].w, sR = c[8].x;
    bool bgDesk = c[8].y > 0.5;
    float shStrength = c[9].x, shSize = c[9].y, bgLevel = c[9].z;

    float2 ndc = float2(i.pos.x / scrSize.x * 2 - 1, 1 - i.pos.y / scrSize.y * 2);
    float2 w = ndc;
    if (warpOn) {
        if (shape == 0) {
            float2 p = float2(ndc.x * aspect, ndc.y);
            float r = length(p);
            float h = warp1(r, R, A);
            p *= r > 1e-5 ? h / r : 1;
            w = float2(p.x / aspect, p.y);
        } else {
            float wx = warp1(abs(ndc.x), R, A) * sign(ndc.x);
            float wy = warp1(abs(ndc.y), R, A) * sign(ndc.y);
            w.x = ((ndc.x > 0 ? sR : sL) > 0.5) ? wx : ndc.x;
            w.y = ((ndc.y > 0 ? sT : sB) > 0.5) ? wy : ndc.y;
        }
    }

    float3 dir = fwd + w.x * th * aspect * right + w.y * th * up;
    bool valid = dir.z < -1e-4;
    float t = valid ? -P.z / dir.z : 0;
    float2 hit = P.xy + t * dir.xy;

    float2 uv = hit / texSize;
    float2 gx = ddx(uv), gy = ddy(uv);
    float2 tx = gx * texSize, ty = gy * texSize;
    float fp = max(length(tx), length(ty));    // texels covered by one screen pixel
    bool inside = valid && all(uv >= 0) && all(uv <= 1);
    float2 cuv = clamp(uv, 0, 1);

    float4 smooth_ = scr.SampleGrad(sAniso, cuv, gx, gy);
    float4 col = smooth_;
    if (filt == 0) {
        float2 tt = uv * texSize;
        float2 q = clamp(sharpUV(tt) / texSize, 0, 1);
        float4 pix = scr.SampleLevel(sLin, q, 0);
        // Minecraft rule: blocky when magnified, mip-filtered when shrunk.
        col = lerp(pix, smooth_, smoothstep(0.9, 1.3, fp));
    }

    float3 bg = bgDesk ? scr.SampleLevel(sLin, i.pos.xy / scrSize, 0).rgb : float3(bgLevel, bgLevel, bgLevel * 1.15);
    // Soft drop shadow under the floating plane (in plane pixels, so it scales with zoom).
    float2 q = hit - float2(0, shSize * 0.35);
    float2 dd = max(max(-q, q - texSize), 0);
    float shadow = valid ? exp(-length(dd) / shSize) * shStrength : 0;
    bg = lerp(bg, float3(0.42, 0.42, 0.44), shadow);
    float3 rgb = inside ? col.rgb : bg;

    float2 local = hit - (M - hot);
    float2 lc = filt == 0 ? sharpUV(local) : local;
    float2 cu = lc / 128.0;
    bool cin = valid && all(cu >= 0) && all(cu <= 1);
    float4 cc = cur.SampleLevel(sLin, clamp(cu, 0, 1), 0);
    float a = (curVis > 0.5 && cin) ? cc.a : 0;
    rgb = lerp(rgb, cc.rgb, a);

    return float4(rgb, 1);
}
