package dev.horizonlab.neoforge;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonPrimitive;
import com.mojang.serialization.Codec;
import java.io.BufferedWriter;
import java.io.IOException;
import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.TreeMap;
import java.util.function.Function;
import java.util.function.Supplier;
import net.minecraft.core.Direction;
import net.minecraft.core.IdMap;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.nbt.NbtAccounter;
import net.minecraft.network.codec.ByteBufCodecs;
import net.minecraft.network.codec.StreamCodec;
import net.minecraft.resources.ResourceKey;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.util.Unit;
import net.minecraft.world.item.ItemStack;
import net.neoforged.neoforge.network.codec.NeoForgeStreamCodecs;
import net.neoforged.neoforge.registries.NeoForgeRegistries;

/**
 * The network StreamCodecs of the server's non-vanilla data component types, entity data
 * serializers and particle types as small trees, so a client that can't decode a mod's value
 * can still read past it ({@code <FILE>.codecs.tsv}: {@code registry TAB id TAB name TAB json}).
 * A leaf is a codec known by identity ({@code "varint"}, {@code "nbt"}, {@code "item"} ...); a
 * factory's codec is recognised by comparing its class with a sample the factory made here and
 * read through its captured fields (names from javap of NeoForge 21.1.265's 1.21.1 jars). What
 * can't be described is {@code {"unknown": class}}. Dev only, never shipped.
 */
final class CodecTrees {
    private static final int MAX_DEPTH = 48;
    private static final int MAX_DISPATCH_CASES = 4096;
    /** {@code ByteBufCodecs.map}'s key and value codecs ({@code ByteBufCodecs$22} decodes the key first). */
    private static final String MAP_KEY = "val$p_331975_";
    private static final String MAP_VALUE = "val$p_331254_";
    /** {@code ByteBufCodecs.either}: {@code true} reads the left codec. */
    private static final String EITHER_LEFT = "val$p_331983_";
    private static final String EITHER_RIGHT = "val$p_332156_";
    private static final String[] COMPOSITE_OWNERS = {
        "net.minecraft.network.codec.StreamCodec$", "net.neoforged.neoforge.network.codec.NeoForgeStreamCodecs$"
    };

    private final RegistryAccess registries;
    private final Map<Object, String> leaves = new IdentityHashMap<>();
    private final Map<Class<?>, String> kinds = new HashMap<>();
    private final Map<Object, Integer> path = new IdentityHashMap<>();
    private final Set<Integer> referenced = new HashSet<>();
    private int nextRec;
    private int unknowns;

    private CodecTrees(RegistryAccess registries) {
        this.registries = registries;
        leaves();
        samples();
    }

    static void write(Path out, RegistryAccess registries) throws IOException {
        CodecTrees trees = new CodecTrees(registries);
        Map<String, int[]> coverage = new TreeMap<>();
        try (BufferedWriter w = Files.newBufferedWriter(out, StandardCharsets.UTF_8)) {
            w.write("# horizon-oracle codecs v1\n");
            trees.registry(w, coverage, BuiltInRegistries.DATA_COMPONENT_TYPE, t -> t.streamCodec(), false);
            trees.registry(w, coverage, NeoForgeRegistries.ENTITY_DATA_SERIALIZERS, s -> s.codec(), true);
            trees.registry(w, coverage, BuiltInRegistries.PARTICLE_TYPE, p -> p.streamCodec(), false);
        }
        coverage.forEach((what, n) ->
                System.out.println("horizon-oracle: codecs " + what + ": " + n[0] + " of " + n[1] + " known"));
    }

    /** One line per entry of {@code registry} outside {@code minecraft} (every entry when {@code all}). */
    private <T> void registry(
            BufferedWriter w, Map<String, int[]> coverage, Registry<T> registry, Function<T, Object> codecOf, boolean all)
            throws IOException {
        String name = registry.key().location().toString();
        for (T value : registry) {
            ResourceLocation key = registry.getKey(value);
            if (key == null || (!all && "minecraft".equals(key.getNamespace()))) {
                continue;
            }
            int before = unknowns;
            JsonElement tree;
            try {
                tree = describe(codecOf.apply(value), 0);
            } catch (RuntimeException e) {
                tree = unknown("codec of " + key + ": " + e);
            }
            int[] n = coverage.computeIfAbsent(name + " " + key.getNamespace(), k -> new int[2]);
            n[0] += unknowns == before ? 1 : 0;
            n[1]++;
            w.write(name + "\t" + registry.getId(value) + "\t" + key + "\t" + tree + "\n");
        }
    }

    private void leaves() {
        leaf(ByteBufCodecs.BOOL, "bool");
        leaf(ByteBufCodecs.BYTE, "byte");
        leaf(ByteBufCodecs.SHORT, "short");
        leaf(ByteBufCodecs.UNSIGNED_SHORT, "short");
        leaf(ByteBufCodecs.INT, "int");
        leaf(ByteBufCodecs.VAR_INT, "varint");
        leaf(ByteBufCodecs.VAR_LONG, "varlong");
        leaf(ByteBufCodecs.FLOAT, "float");
        leaf(ByteBufCodecs.DOUBLE, "double");
        leaf(ByteBufCodecs.BYTE_ARRAY, "bytes");
        leaf(ByteBufCodecs.STRING_UTF8, "string");
        leaf(ByteBufCodecs.TAG, "nbt");
        leaf(ByteBufCodecs.TRUSTED_TAG, "nbt");
        leaf(ByteBufCodecs.COMPOUND_TAG, "nbt");
        leaf(ByteBufCodecs.TRUSTED_COMPOUND_TAG, "nbt");
        // FriendlyByteBuf.readNbt: one tag, an End tag for none.
        leaf(ByteBufCodecs.OPTIONAL_COMPOUND_TAG, "nbt");
        leaf(ByteBufCodecs.VECTOR3F, "vec3f");
        leaf(ByteBufCodecs.QUATERNIONF, "quaternion");
        leaf(ByteBufCodecs.GAME_PROFILE_PROPERTIES, "properties");
        leaf(ByteBufCodecs.GAME_PROFILE, "game_profile");
        leaf(NeoForgeStreamCodecs.UNBOUNDED_BYTE_ARRAY, "bytes");
        leaf(NeoForgeStreamCodecs.CHUNK_POS, "long");
        leaf("net.minecraft.resources.ResourceLocation", "STREAM_CODEC", "string");
        leaf("net.minecraft.core.UUIDUtil", "STREAM_CODEC", "uuid");
        leaf("net.minecraft.core.BlockPos", "STREAM_CODEC", "long");
        leaf("net.minecraft.network.chat.ComponentSerialization", "STREAM_CODEC", "nbt");
        leaf("net.minecraft.network.chat.ComponentSerialization", "TRUSTED_STREAM_CODEC", "nbt");
        leaf("net.minecraft.network.chat.ComponentSerialization", "TRUSTED_CONTEXT_FREE_STREAM_CODEC", "nbt");
        // ItemStack.STREAM_CODEC is the optional one refusing an empty stack: the same bytes.
        leaf("net.minecraft.world.item.ItemStack", "STREAM_CODEC", "item");
        leaf("net.minecraft.world.item.ItemStack", "OPTIONAL_STREAM_CODEC", "item");
        leaf("net.minecraft.core.component.DataComponentPatch", "STREAM_CODEC", "patch");
        leaf("net.neoforged.neoforge.fluids.FluidStack", "STREAM_CODEC", "fluid");
        leaf("net.neoforged.neoforge.fluids.FluidStack", "OPTIONAL_STREAM_CODEC", "fluid");
    }

    private void leaf(Object codec, String name) {
        if (codec != null) {
            leaves.put(codec, name);
        }
    }

    private void leaf(String owner, String field, String name) {
        try {
            leaf(Class.forName(owner, true, CodecTrees.class.getClassLoader()).getField(field).get(null), name);
        } catch (ReflectiveOperationException | LinkageError e) {
            System.out.println("horizon-oracle: no " + owner + "." + field + ": " + e);
        }
    }

    /** The factories' codec classes, from one codec each makes here. */
    @SuppressWarnings({"rawtypes", "unchecked"})
    private void samples() {
        StreamCodec b = ByteBufCodecs.BOOL;
        Function same = x -> x;
        sample(() -> ByteBufCodecs.byteArray(1), "bytes");
        sample(() -> ByteBufCodecs.stringUtf8(1), "string");
        sample(() -> ByteBufCodecs.tagCodec(NbtAccounter::unlimitedHeap), "nbt");
        sample(() -> ByteBufCodecs.fromCodecWithRegistries(Codec.INT), "nbt");
        sample(() -> ByteBufCodecs.optional(b), "optional");
        sample(() -> ByteBufCodecs.collection(ArrayList::new, b), "list");
        sample(() -> ByteBufCodecs.map(HashMap::new, b, b), "map");
        sample(() -> ByteBufCodecs.either(b, b), "either");
        sample(() -> ByteBufCodecs.idMapper(i -> null, o -> 0), "varint");
        sample(() -> ByteBufCodecs.registry(Registries.ITEM), "registry");
        sample(() -> ByteBufCodecs.holderRegistry(Registries.ITEM), "registry");
        sample(() -> ByteBufCodecs.holder(Registries.ITEM, b), "holder");
        sample(() -> ByteBufCodecs.holderSet(Registries.ITEM), "holder_set");
        sample(() -> StreamCodec.of((buf, v) -> {}, buf -> null), "of");
        sample(() -> StreamCodec.ofMember((v, buf) -> {}, buf -> null), "of");
        sample(() -> StreamCodec.unit(Unit.INSTANCE), "unit");
        sample(() -> b.map(same, same), "wrap");
        sample(() -> b.mapStream(same), "wrap");
        sample(() -> b.dispatch(same, same), "dispatch");
        sample(() -> StreamCodec.recursive(c -> b), "recursive");
        sample(() -> ItemStack.validatedStreamCodec(ItemStack.OPTIONAL_STREAM_CODEC), "validated");
        sample(() -> NeoForgeStreamCodecs.lazy(() -> b), "lazy");
        sample(() -> NeoForgeStreamCodecs.enumCodec(Direction.class), "varint");
        sample(() -> NeoForgeStreamCodecs.registryKey(), "string");
        sample(() -> NeoForgeStreamCodecs.connectionAware(b, b), "connection");
        sample(() -> NeoForgeStreamCodecs.uncheckedUnit(Unit.INSTANCE), "unit");
    }

    private void sample(Supplier<Object> make, String kind) {
        try {
            kinds.putIfAbsent(make.get().getClass(), kind);
        } catch (RuntimeException | LinkageError e) {
            System.out.println("horizon-oracle: no sample codec for " + kind + ": " + e);
        }
    }

    /** {@code codec} as a tree; a codec met again inside itself is {@code {"self": n}} of its {@code {"rec": n}}. */
    private JsonElement describe(Object codec, int depth) {
        if (codec == null) {
            return unknown("null codec");
        }
        String leaf = leaves.get(codec);
        if (leaf != null) {
            return new JsonPrimitive(leaf);
        }
        Integer outer = path.get(codec);
        if (outer != null) {
            referenced.add(outer);
            return single("self", new JsonPrimitive(outer));
        }
        if (depth > MAX_DEPTH) {
            return unknown("too deep at " + codec.getClass().getName());
        }
        int rec = nextRec++;
        path.put(codec, rec);
        JsonElement tree;
        try {
            tree = structure(codec, depth + 1);
        } catch (ReflectiveOperationException | RuntimeException e) {
            tree = unknown(codec.getClass().getName() + ": " + e);
        } finally {
            path.remove(codec);
        }
        if (!referenced.remove(rec)) {
            return tree;
        }
        JsonObject o = new JsonObject();
        o.addProperty("rec", rec);
        o.add("body", tree);
        return o;
    }

    @SuppressWarnings({"rawtypes", "unchecked"})
    private JsonElement structure(Object codec, int depth) throws ReflectiveOperationException {
        Class<?> type = codec.getClass();
        String kind = kinds.get(type);
        if (kind == null && composite(type)) {
            kind = "seq";
        }
        if (kind == null) {
            return unknown(type.getName());
        }
        switch (kind) {
            case "bytes", "string", "nbt", "varint", "unit", "holder_set":
                return new JsonPrimitive(kind);
            case "registry":
                return new JsonPrimitive("varint");
            case "optional", "list", "holder":
                return single(kind, describe(only(codec), depth));
            case "map":
                return pair("map", named(codec, MAP_KEY, 0), named(codec, MAP_VALUE, 1), depth);
            case "either":
                return pair("either", named(codec, EITHER_LEFT, 0), named(codec, EITHER_RIGHT, 1), depth);
            case "wrap":
                return describe(field(codec, "this$0"), depth);
            case "validated":
                return describe(only(codec), depth);
            case "lazy":
                return describe(((Supplier) field(codec, "delegate")).get(), depth);
            case "recursive":
                return describe(((Supplier) field(codec, "inner")).get(), depth);
            case "connection":
                // Horizon answers neoforge:register, so the server writes the NeoForge branch.
                return describe(field(codec, "val$neoForgeCodec"), depth);
            case "dispatch":
                return dispatch(codec, depth);
            case "seq":
                return seq(codec, depth);
            default:
                return unknown(kind + " " + type.getName() + decoder(codec));
        }
    }

    /** {@code StreamCodec.composite} (1-6) and {@code NeoForgeStreamCodecs.composite} (7): {@code val$codec1..N}. */
    private static boolean composite(Class<?> type) {
        for (String owner : COMPOSITE_OWNERS) {
            if (type.getName().startsWith(owner)) {
                try {
                    type.getDeclaredField("val$codec1");
                    return true;
                } catch (NoSuchFieldException e) {
                    return false;
                }
            }
        }
        return false;
    }

    private JsonElement seq(Object codec, int depth) throws ReflectiveOperationException {
        JsonArray fields = new JsonArray();
        for (int i = 1; ; i++) {
            Field f;
            try {
                f = codec.getClass().getDeclaredField("val$codec" + i);
            } catch (NoSuchFieldException e) {
                break;
            }
            f.setAccessible(true);
            fields.add(describe(f.get(codec), depth));
        }
        return single("seq", fields);
    }

    /**
     * {@code keyCodec.dispatch(type, codec)}: a case per key when the key is a registry entry's id
     * ({@code ByteBufCodecs.registry} / {@code holderRegistry}), each the codec its entry selects.
     */
    @SuppressWarnings({"rawtypes", "unchecked"})
    private JsonElement dispatch(Object codec, int depth) throws ReflectiveOperationException {
        Object keyCodec = field(codec, "this$0");
        Function codecs = (Function) field(codec, "val$codec");
        if (!"registry".equals(kinds.get(keyCodec.getClass()))) {
            return unknown("dispatch on " + keyCodec.getClass().getName());
        }
        ResourceKey key = (ResourceKey) ofType(keyCodec, ResourceKey.class);
        Function idGetter = (Function) ofType(keyCodec, Function.class);
        Optional<Registry> registry = (Optional) registries.registry(key);
        Registry found = registry.orElseGet(() -> (Registry) BuiltInRegistries.REGISTRY.get(key.location()));
        if (found == null || idGetter == null) {
            return unknown("dispatch over missing registry " + key);
        }
        IdMap ids = (IdMap) idGetter.apply(found);
        JsonObject cases = new JsonObject();
        int n = 0;
        for (Object value : ids) {
            if (++n > MAX_DISPATCH_CASES) {
                return unknown("dispatch over more than " + MAX_DISPATCH_CASES + " of " + key);
            }
            JsonElement tree;
            try {
                tree = describe(codecs.apply(value), depth);
            } catch (RuntimeException e) {
                tree = unknown("dispatch case of " + key + ": " + e);
            }
            cases.add(String.valueOf(ids.getId(value)), tree);
        }
        return single("dispatch", cases);
    }

    private JsonElement pair(String kind, Object first, Object second, int depth) {
        JsonArray both = new JsonArray();
        both.add(describe(first, depth));
        both.add(describe(second, depth));
        return single(kind, both);
    }

    private JsonElement unknown(String what) {
        unknowns++;
        return single("unknown", new JsonPrimitive(what));
    }

    private static JsonObject single(String key, JsonElement value) {
        JsonObject o = new JsonObject();
        o.add(key, value);
        return o;
    }

    /** {@code of}/{@code ofMember}'s decoder class, which names the mod's own reader. */
    private static String decoder(Object codec) {
        try {
            return " decoder " + field(codec, "val$decoder").getClass().getName();
        } catch (ReflectiveOperationException | RuntimeException e) {
            return "";
        }
    }

    private static Object field(Object o, String name) throws ReflectiveOperationException {
        Field f = o.getClass().getDeclaredField(name);
        f.setAccessible(true);
        return f.get(o);
    }

    /** The field {@code name}, else the {@code index}th StreamCodec field in declaration order. */
    private static Object named(Object o, String name, int index) throws ReflectiveOperationException {
        try {
            return field(o, name);
        } catch (NoSuchFieldException e) {
            List<Object> codecs = codecFields(o);
            return index < codecs.size() ? codecs.get(index) : null;
        }
    }

    /** The one StreamCodec a wrapper captured. */
    private static Object only(Object o) throws ReflectiveOperationException {
        List<Object> codecs = codecFields(o);
        if (codecs.size() != 1) {
            throw new NoSuchFieldException(codecs.size() + " codec fields in " + o.getClass().getName());
        }
        return codecs.get(0);
    }

    private static List<Object> codecFields(Object o) throws ReflectiveOperationException {
        List<Object> out = new ArrayList<>();
        for (Field f : o.getClass().getDeclaredFields()) {
            if (!Modifier.isStatic(f.getModifiers()) && StreamCodec.class.isAssignableFrom(f.getType())) {
                f.setAccessible(true);
                out.add(f.get(o));
            }
        }
        return out;
    }

    private static Object ofType(Object o, Class<?> type) throws ReflectiveOperationException {
        for (Field f : o.getClass().getDeclaredFields()) {
            if (!Modifier.isStatic(f.getModifiers()) && type.isAssignableFrom(f.getType())) {
                f.setAccessible(true);
                return f.get(o);
            }
        }
        return null;
    }
}
