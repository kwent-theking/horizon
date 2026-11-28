package dev.horizonlab.modeldump;

import com.google.gson.stream.JsonWriter;
import java.io.IOException;
import java.io.StringWriter;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.TreeMap;
import net.minecraft.client.model.EntityModel;
import net.minecraft.client.model.geom.ModelPart;
import net.minecraft.world.entity.Entity;

/**
 * What a model's real {@code setupAnim} does, sampled: the model is posed over a small grid of
 * inputs and each part's change from its dumped pose is written for the Horizon client to
 * interpolate (crates/horizon-client/src/mobs/modded.rs), as the entity's optional {@code sampled}:
 *
 * <pre>
 * {"approximate": true,                    // only when the walk cycle is not periodic
 *  "walk": {"period": limbSwing period, "amplitudes": [0, 0.5, 1], "phases": 16,
 *           "parts": {name: samples}},     // sample index = amplitude * phases + phase
 *  "idle": {"period": ageInTicks period, "steps": 8, "parts": {name: samples}},
 *  "head": {"yaw": [...], "pitch": [...],  // degrees; deltas from the yaw 0, pitch 0 pose
 *           "parts": {name: samples}}}     // sample index = yaw index * pitch count + pitch index
 * </pre>
 *
 * A sample is {@code x, y, z, xRot, yRot, zRot} added to the part's dumped pose (pixels, radians). A
 * part's list holds one sample when it is the same everywhere in its group, else one per grid point.
 * Parts that never move are left out, and so are groups with no moving part. Walk samples are taken
 * with ageInTicks 0, idle samples with limbSwingAmount 0, head samples with both 0. Part names are the
 * ones the Horizon client gives the layer's tree (children by name, repeats {@code name#2}...).
 */
final class SetupAnimSampler {
    private static final float LIMB_RATE = 0.6662F;
    private static final float WALK_PERIOD = (float) (2 * Math.PI / LIMB_RATE);
    private static final int WALK_PHASES = 16;
    private static final float[] AMPLITUDES = {0, 0.5F, 1};
    private static final int IDLE_STEPS = 8;
    private static final float IDLE_PERIOD = 40;
    private static final float[] HEAD_YAW = {-45, 0, 45};
    private static final float[] HEAD_PITCH = {-30, 0, 30};
    /** Values below this are unchanged (the output keeps four decimals). */
    private static final float EPSILON = 0.00005F;
    private static final int CHANNELS = 6;

    private record Named(String name, ModelPart part) {}

    private record Saved(float[] pose, boolean visible, boolean skipDraw) {}

    private final EntityModel<Entity> model;
    private final Entity entity;
    private final List<Named> parts;
    private final List<Saved> saved = new ArrayList<>();

    @SuppressWarnings("unchecked")
    private SetupAnimSampler(EntityModel<?> model, Entity entity, List<Named> parts) {
        this.model = (EntityModel<Entity>) model;
        this.entity = entity;
        this.parts = parts;
    }

    /** The {@code sampled} JSON, or null when the model can't be sampled or nothing in it moves. */
    static String sample(Object model, Object root, Entity entity) {
        if (!(model instanceof EntityModel<?> entityModel) || entity == null || root == null) {
            return null;
        }
        SetupAnimSampler sampler = null;
        try {
            List<Named> parts = named(root);
            if (parts.isEmpty()) {
                return null;
            }
            sampler = new SetupAnimSampler(entityModel, entity, parts);
            return sampler.run();
        } catch (Throwable error) {
            HorizonModelDump.LOG.debug("horizon-model-dump: setupAnim of {}: {}", model.getClass().getName(),
                error.toString());
            return null;
        } finally {
            if (sampler != null) {
                sampler.restoreAll();
            }
        }
    }

    private String run() throws IOException {
        for (Named named : parts) {
            ModelPart p = named.part();
            saved.add(new Saved(new float[] {p.x, p.y, p.z, p.xRot, p.yRot, p.zRot, p.xScale, p.yScale, p.zScale},
                p.visible, p.skipDraw));
        }
        List<float[][]> walk = new ArrayList<>();
        for (float amplitude : AMPLITUDES) {
            for (int phase = 0; phase < WALK_PHASES; phase++) {
                walk.add(capture(WALK_PERIOD * phase / WALK_PHASES, amplitude, 0, 0, 0));
            }
        }
        float[][] wrapped = capture(WALK_PERIOD, AMPLITUDES[AMPLITUDES.length - 1], 0, 0, 0);
        boolean approximate = differs(wrapped, walk.get((AMPLITUDES.length - 1) * WALK_PHASES));
        List<float[][]> idle = new ArrayList<>();
        for (int step = 0; step < IDLE_STEPS; step++) {
            idle.add(capture(0, 0, IDLE_PERIOD * step / IDLE_STEPS, 0, 0));
        }
        float[][] center = capture(0, 0, 0, 0, 0);
        List<float[][]> head = new ArrayList<>();
        for (float yaw : HEAD_YAW) {
            for (float pitch : HEAD_PITCH) {
                head.add(minus(capture(0, 0, 0, yaw, pitch), center));
            }
        }
        StringWriter out = new StringWriter();
        boolean any = false;
        try (JsonWriter w = new JsonWriter(out)) {
            w.beginObject();
            if (approximate) {
                w.name("approximate").value(true);
            }
            any |= group(w, "walk", walk, g -> {
                g.name("period").jsonValue(num(WALK_PERIOD));
                g.name("amplitudes").beginArray();
                for (float amplitude : AMPLITUDES) {
                    g.jsonValue(num(amplitude));
                }
                g.endArray();
                g.name("phases").value(WALK_PHASES);
            });
            any |= group(w, "idle", idle, g -> {
                g.name("period").jsonValue(num(IDLE_PERIOD));
                g.name("steps").value(IDLE_STEPS);
            });
            any |= group(w, "head", head, g -> {
                g.name("yaw").beginArray();
                for (float yaw : HEAD_YAW) {
                    g.jsonValue(num(yaw));
                }
                g.endArray();
                g.name("pitch").beginArray();
                for (float pitch : HEAD_PITCH) {
                    g.jsonValue(num(pitch));
                }
                g.endArray();
            });
            w.endObject();
        }
        return any ? out.toString() : null;
    }

    private interface Header {
        void write(JsonWriter w) throws IOException;
    }

    /** One group: its header and the parts that move in any of its samples; false when none do. */
    private boolean group(JsonWriter w, String name, List<float[][]> samples, Header header) throws IOException {
        StringWriter body = new StringWriter();
        boolean any = false;
        try (JsonWriter p = new JsonWriter(body)) {
            p.beginObject();
            for (int i = 0; i < parts.size(); i++) {
                if (!moves(samples, i)) {
                    continue;
                }
                any = true;
                p.name(parts.get(i).name()).beginArray();
                boolean constant = true;
                for (float[][] sample : samples) {
                    constant &= !differs(sample[i], samples.get(0)[i]);
                }
                for (int s = 0; s < (constant ? 1 : samples.size()); s++) {
                    for (float v : samples.get(s)[i]) {
                        p.jsonValue(num(v));
                    }
                }
                p.endArray();
            }
            p.endObject();
        }
        if (!any) {
            return false;
        }
        w.name(name).beginObject();
        header.write(w);
        w.name("parts").jsonValue(body.toString());
        w.endObject();
        return true;
    }

    private static boolean moves(List<float[][]> samples, int part) {
        for (float[][] sample : samples) {
            for (float v : sample[part]) {
                if (Math.abs(v) >= EPSILON) {
                    return true;
                }
            }
        }
        return false;
    }

    private static boolean differs(float[] a, float[] b) {
        for (int i = 0; i < a.length; i++) {
            if (Math.abs(a[i] - b[i]) >= EPSILON) {
                return true;
            }
        }
        return false;
    }

    private static boolean differs(float[][] a, float[][] b) {
        for (int i = 0; i < a.length; i++) {
            if (differs(a[i], b[i])) {
                return true;
            }
        }
        return false;
    }

    private static float[][] minus(float[][] a, float[][] b) {
        float[][] out = new float[a.length][CHANNELS];
        for (int i = 0; i < a.length; i++) {
            for (int c = 0; c < CHANNELS; c++) {
                out[i][c] = a[i][c] - b[i][c];
            }
        }
        return out;
    }

    /** Every part back to its dumped pose, setupAnim, then each part's change from that pose. */
    private float[][] capture(float limbSwing, float limbSwingAmount, float ageInTicks, float netHeadYaw,
            float headPitch) {
        restoreAll();
        model.setupAnim(entity, limbSwing, limbSwingAmount, ageInTicks, netHeadYaw, headPitch);
        float[][] out = new float[parts.size()][CHANNELS];
        for (int i = 0; i < parts.size(); i++) {
            ModelPart p = parts.get(i).part();
            float[] base = saved.get(i).pose();
            float[] now = {p.x, p.y, p.z, p.xRot, p.yRot, p.zRot};
            for (int c = 0; c < CHANNELS; c++) {
                float delta = now[c] - base[c];
                if (!Float.isFinite(delta)) {
                    throw new IllegalStateException("non-finite pose of " + parts.get(i).name());
                }
                out[i][c] = delta;
            }
        }
        return out;
    }

    private void restoreAll() {
        for (int i = 0; i < saved.size(); i++) {
            ModelPart p = parts.get(i).part();
            Saved s = saved.get(i);
            float[] v = s.pose();
            p.x = v[0];
            p.y = v[1];
            p.z = v[2];
            p.xRot = v[3];
            p.yRot = v[4];
            p.zRot = v[5];
            p.xScale = v[6];
            p.yScale = v[7];
            p.zScale = v[8];
            p.visible = s.visible();
            p.skipDraw = s.skipDraw();
        }
    }

    /** Four decimals, plain JSON number text. */
    private static String num(float f) {
        double rounded = Math.round(f * 10000.0) / 10000.0;
        return rounded == 0 ? "0" : Double.toString(rounded);
    }

    /**
     * The parts of the layer tree {@code root} in the names the Horizon client gives them: depth first,
     * children by name, a repeated name {@code name#2}..., the root "root" (or "" when it has a child
     * "root").
     */
    private static List<Named> named(Object root) throws ReflectiveOperationException {
        List<Named> out = new ArrayList<>();
        Set<String> seen = new HashSet<>();
        if (root instanceof ModelPart part) {
            Map<String, ModelPart> children = PartJson.children(part);
            collect(children.containsKey("root") ? "" : "root", part, seen, out);
        } else if (root instanceof PartJson.Fields fields) {
            Map<String, ModelPart> children = new TreeMap<>();
            for (Map.Entry<String, ModelPart> entry : fields.parts()) {
                children.put(entry.getKey(), entry.getValue());
            }
            seen.add(children.containsKey("root") ? "" : "root");
            for (Map.Entry<String, ModelPart> child : children.entrySet()) {
                collect(child.getKey(), child.getValue(), seen, out);
            }
        }
        return out;
    }

    private static void collect(String name, ModelPart part, Set<String> seen, List<Named> out)
            throws ReflectiveOperationException {
        String unique = name;
        if (!seen.add(unique)) {
            int n = 2;
            while (seen.contains(name + "#" + n)) {
                n++;
            }
            unique = name + "#" + n;
            seen.add(unique);
        }
        out.add(new Named(unique, part));
        for (Map.Entry<String, ModelPart> child : new TreeMap<>(PartJson.children(part)).entrySet()) {
            collect(child.getKey(), child.getValue(), seen, out);
        }
    }
}
