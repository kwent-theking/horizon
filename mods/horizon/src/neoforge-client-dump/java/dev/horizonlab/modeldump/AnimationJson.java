package dev.horizonlab.modeldump;

import com.google.gson.stream.JsonWriter;
import java.io.IOException;
import java.io.StringWriter;
import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.util.HashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.TreeMap;
import org.joml.Vector3f;

/**
 * The keyframe clips a model plays: the static {@code AnimationDefinition}s (vanilla's, or
 * LionfishAPI's {@code AdvancedAnimationDefinition} of the same record shape) that its class and its
 * mod superclasses read, found through their constant pools. Written in Horizon's keyframe format
 * (crates/horizon-client/src/mobs/keyframes.rs): {@code length}, {@code looping} and per bone a list
 * of {@code {target, keyframes: [[time, pre, post, interpolation]]}}; 1.21.1 keyframes have one
 * target, so pre and post are the same. Values are as stored: rotations in radians, positions with
 * y already flipped, scales as offsets from 1 (KeyframeAnimations.degreeVec / posVec / scaleVec).
 */
final class AnimationJson {
    private static final Map<Class<?>, Map<Object, String>> NAMES = new HashMap<>();

    private AnimationJson() {}

    /** Clip name (the field's name, lower case) to its definition. */
    static Map<String, Object> of(Class<?> modelClass) {
        Map<String, Object> out = new TreeMap<>();
        for (Class<?> c : Reflect.modClasses(modelClass)) {
            for (ClassConstants.FieldRef ref : ClassConstants.of(c).fields) {
                if (!ref.descriptor().endsWith("AnimationDefinition;")) {
                    continue;
                }
                Object definition = staticValue(c, ref);
                if (definition == null || Reflect.call(definition, "boneAnimations") == null) {
                    continue;
                }
                String name = ref.name().toLowerCase(Locale.ROOT);
                Object known = out.get(name);
                if (known != null && known != definition) {
                    name = ref.owner().substring(ref.owner().lastIndexOf('.') + 1).toLowerCase(Locale.ROOT) + "." + name;
                }
                out.putIfAbsent(name, definition);
            }
        }
        return out;
    }

    private static Object staticValue(Class<?> user, ClassConstants.FieldRef ref) {
        try {
            Class<?> owner = Class.forName(ref.owner(), true, user.getClassLoader());
            for (Class<?> c = owner; c != null; c = c.getSuperclass()) {
                for (Field f : c.getDeclaredFields()) {
                    if (f.getName().equals(ref.name()) && Modifier.isStatic(f.getModifiers())) {
                        f.setAccessible(true);
                        return f.get(null);
                    }
                }
            }
        } catch (ReflectiveOperationException | RuntimeException | LinkageError error) {
            HorizonModelDump.LOG.debug("horizon-model-dump: clip {}.{}: {}", ref.owner(), ref.name(), error.toString());
        }
        return null;
    }

    /** The clip as JSON, or null when its shape is not the one expected. */
    static String json(Object definition) {
        try {
            StringWriter out = new StringWriter();
            try (JsonWriter w = new JsonWriter(out)) {
                write(w, definition);
            }
            return out.toString();
        } catch (IOException | RuntimeException error) {
            HorizonModelDump.LOG.debug("horizon-model-dump: unreadable clip {}: {}", definition, error.toString());
            return null;
        }
    }

    private static void write(JsonWriter w, Object definition) throws IOException {
        float length = (Float) Reflect.call(definition, "lengthInSeconds");
        boolean looping = (Boolean) Reflect.call(definition, "looping");
        Map<?, ?> bones = (Map<?, ?>) Reflect.call(definition, "boneAnimations");
        w.beginObject();
        w.name("length");
        PartJson.number(w, length);
        w.name("looping").value(looping);
        w.name("bones").beginObject();
        for (Map.Entry<?, ?> bone : bones.entrySet()) {
            w.name(String.valueOf(bone.getKey())).beginArray();
            for (Object channel : (List<?>) bone.getValue()) {
                String target = name(channel.getClass(), "Targets", Reflect.call(channel, "target"));
                if (target == null) {
                    continue;
                }
                w.beginObject();
                w.name("target").value(target);
                w.name("keyframes").beginArray();
                for (Object keyframe : (Object[]) Reflect.call(channel, "keyframes")) {
                    Vector3f value = (Vector3f) Reflect.call(keyframe, "target");
                    String interpolation = name(channel.getClass(), "Interpolations", Reflect.call(keyframe, "interpolation"));
                    w.beginArray();
                    PartJson.number(w, (Float) Reflect.call(keyframe, "timestamp"));
                    PartJson.vector(w, value);
                    PartJson.vector(w, value);
                    w.value("catmullrom".equals(interpolation) ? "catmullrom" : "linear");
                    w.endArray();
                }
                w.endArray();
                w.endObject();
            }
            w.endArray();
        }
        w.endObject();
        w.endObject();
    }

    /**
     * Which constant of the channel's nested {@code holder} class (Targets: POSITION, ROTATION,
     * SCALE; Interpolations: LINEAR, CATMULLROM) {@code value} is, lower case; null for another.
     */
    private static String name(Class<?> channel, String holder, Object value) {
        if (value == null) {
            return null;
        }
        Map<Object, String> names = NAMES.computeIfAbsent(holderClass(channel, holder), AnimationJson::constants);
        return names.get(value);
    }

    private static Class<?> holderClass(Class<?> channel, String holder) {
        for (Class<?> c : channel.getDeclaredClasses()) {
            if (c.getSimpleName().equals(holder)) {
                return c;
            }
        }
        return Void.class;
    }

    private static Map<Object, String> constants(Class<?> holder) {
        Map<Object, String> out = new java.util.IdentityHashMap<>();
        for (Field f : holder.getDeclaredFields()) {
            if (Modifier.isStatic(f.getModifiers())) {
                try {
                    f.setAccessible(true);
                    Object value = f.get(null);
                    if (value != null) {
                        out.put(value, f.getName().toLowerCase(Locale.ROOT));
                    }
                } catch (ReflectiveOperationException | RuntimeException error) {
                    // A constant we can't read is never matched.
                }
            }
        }
        return out;
    }
}
