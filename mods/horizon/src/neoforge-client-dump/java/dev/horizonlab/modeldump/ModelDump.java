package dev.horizonlab.modeldump;

import com.google.gson.stream.JsonWriter;
import java.io.IOException;
import java.io.InputStream;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.AtomicMoveNotSupportedException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import java.util.TreeMap;
import net.minecraft.client.Minecraft;
import net.minecraft.client.model.geom.ModelPart;
import net.minecraft.client.renderer.entity.EntityRenderDispatcher;
import net.minecraft.client.renderer.entity.EntityRenderer;
import net.minecraft.client.renderer.entity.EntityRendererProvider;
import net.minecraft.client.renderer.entity.EntityRenderers;
import net.minecraft.client.renderer.entity.LivingEntityRenderer;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.packs.resources.Resource;
import net.minecraft.server.packs.resources.ResourceManager;
import net.minecraft.world.entity.EntityType;

/**
 * Every modded entity type's renderer, built again with a {@link RecordingModelSet} so the layer its
 * model was baked from is known, written as {@code modded-entity-models.json}:
 *
 * <pre>
 * {"format": 1,
 *  "entities": {"ns:id": {"renderer": class, "system": how the model was found, "living": bool,
 *                         "layer": key in models, "texture": "ns:textures/....png",
 *                         "width", "height", "eye_height", "animations": {clip: keyframes}, "error"}},
 *  "models": {"ns:path#layer": part tree, as in Horizon's entity-models.json}}
 * </pre>
 *
 * "system" is "layer" (the recorded layer holding the model's parts), "model" (a hierarchical
 * model's root), "fields" (the model's own part fields), "first-layer", "lionfish" (a box tree),
 * "geckolib" (drawn from GeckoLib files, no model here), "none" or "no-renderer". The textures of the
 * living entities with a model are copied to {@code modded-entity-textures/<ns>/textures/...} beside
 * the JSON, so the Horizon client can pack them without the mods' jars.
 */
final class ModelDump {
    private static final String TEXTURES = "modded-entity-textures";

    private final Minecraft mc;
    private final ResourceManager resources;

    private static final class Entry {
        final ResourceLocation id;
        String renderer;
        String system = "none";
        boolean living;
        String layer;
        Object root;
        Map<Object, String> boxNames = Map.of();
        ResourceLocation texture;
        float width;
        float height;
        float eyeHeight;
        Map<String, Object> animations = Map.of();
        String error;

        Entry(ResourceLocation id) {
            this.id = id;
        }
    }

    ModelDump(Minecraft mc) {
        this.mc = mc;
        this.resources = mc.getResourceManager();
    }

    /** Writes the JSON to {@code out} and the textures beside it; returns a summary line. */
    String write(Path out) throws IOException, ReflectiveOperationException {
        @SuppressWarnings("unchecked")
        Map<EntityType<?>, EntityRendererProvider<?>> providers =
            (Map<EntityType<?>, EntityRendererProvider<?>>) Reflect.getStatic(EntityRenderers.class, "PROVIDERS");
        EntityRenderDispatcher dispatcher = mc.getEntityRenderDispatcher();
        Map<?, ?> existing = Reflect.get(dispatcher, "renderers") instanceof Map<?, ?> map ? map : Map.of();
        RecordingModelSet models = new RecordingModelSet(mc.getEntityModels());
        EntityRendererProvider.Context context = new EntityRendererProvider.Context(dispatcher, mc.getItemRenderer(),
            mc.getBlockRenderer(), dispatcher.getItemInHandRenderer(), resources, models, mc.font);
        List<Entry> entries = new ArrayList<>();
        for (EntityType<?> type : BuiltInRegistries.ENTITY_TYPE) {
            ResourceLocation id = BuiltInRegistries.ENTITY_TYPE.getKey(type);
            if (id.getNamespace().equals(ResourceLocation.DEFAULT_NAMESPACE)) {
                continue;
            }
            Entry entry = new Entry(id);
            try {
                describe(entry, type, providers.get(type), existing.get(type), context, models);
            } catch (Throwable error) {
                entry.error = String.valueOf(error);
            }
            models.take();
            entries.add(entry);
        }
        entries.sort(Comparator.comparing(e -> e.id.toString()));
        Map<String, String> trees = trees(entries);
        int textures = copyTextures(entries, trees, out.toAbsolutePath().resolveSibling(TEXTURES));
        writeJson(out, entries, trees);
        return summary(entries, trees, textures);
    }

    private void describe(Entry e, EntityType<?> type, EntityRendererProvider<?> provider, Object existing,
            EntityRendererProvider.Context context, RecordingModelSet models) {
        e.width = type.getWidth();
        e.height = type.getHeight();
        e.eyeHeight = type.getDimensions().eyeHeight();
        EntityRenderer<?> renderer = null;
        List<RecordingModelSet.Baked> baked = List.of();
        if (provider != null) {
            models.take();
            try {
                renderer = provider.create(context);
            } catch (Throwable error) {
                e.error = "renderer: " + error;
            }
            baked = models.take();
        }
        if (renderer == null && existing instanceof EntityRenderer<?> built) {
            renderer = built;
        }
        if (renderer == null) {
            e.system = provider == null ? "no-renderer" : "none";
            return;
        }
        e.renderer = renderer.getClass().getName();
        if (Reflect.hierarchyStartsWith(renderer.getClass(), "software.bernie.geckolib")) {
            e.system = "geckolib";
        }
        Object model = null;
        if (renderer instanceof LivingEntityRenderer<?, ?> living) {
            e.living = true;
            model = living.getModel();
        }
        if (model != null) {
            resolve(e, model, baked);
            e.animations = AnimationJson.of(model.getClass());
        }
        e.texture = Textures.of(renderer, model, e.id, type, resources);
    }

    /** Which part tree draws the model, and under which key. */
    private static void resolve(Entry e, Object model, List<RecordingModelSet.Baked> baked) {
        Object root = Reflect.call(model, "root");
        if (root != null && !(root instanceof ModelPart) && PartJson.isBox(root)) {
            e.system = "lionfish";
            e.layer = e.id + "#lionfish";
            e.root = root;
            e.boxNames = PartJson.fieldNames(model);
            return;
        }
        Map<String, ModelPart> fields = PartJson.modelParts(model);
        Set<ModelPart> held = Reflect.identitySet();
        held.addAll(fields.values());
        if (root instanceof ModelPart part) {
            held.add(part);
        }
        RecordingModelSet.Baked best = null;
        long bestCount = 0;
        for (RecordingModelSet.Baked layer : baked) {
            long count = layer.root().getAllParts().filter(held::contains).count();
            if (count > bestCount) {
                best = layer;
                bestCount = count;
            }
        }
        if (best != null) {
            e.system = "layer";
            e.layer = best.location().toString();
            e.root = best.root();
        } else if (root instanceof ModelPart part) {
            e.system = "model";
            e.layer = e.id + "#model";
            e.root = part;
        } else if (!fields.isEmpty()) {
            e.system = "fields";
            e.layer = e.id + "#fields";
            e.root = new PartJson.Fields(PartJson.tops(fields));
        } else if (!baked.isEmpty()) {
            e.system = "first-layer";
            e.layer = baked.get(0).location().toString();
            e.root = baked.get(0).root();
        }
    }

    /** Each layer's JSON once; an entity whose tree can't be written loses its layer. */
    private static Map<String, String> trees(List<Entry> entries) {
        Map<String, String> out = new LinkedHashMap<>();
        Set<String> failed = new HashSet<>();
        for (Entry e : entries) {
            if (e.layer == null || e.root == null || out.containsKey(e.layer)) {
                continue;
            }
            if (!failed.contains(e.layer)) {
                try {
                    out.put(e.layer, PartJson.json(e.root, e.boxNames));
                    continue;
                } catch (Throwable error) {
                    failed.add(e.layer);
                    e.error = "model: " + error;
                }
            }
            e.layer = null;
        }
        return out;
    }

    private static boolean drawable(Entry e, Map<String, String> trees) {
        return e.living && e.layer != null && trees.containsKey(e.layer) && e.texture != null;
    }

    private int copyTextures(List<Entry> entries, Map<String, String> trees, Path dir) throws IOException {
        Set<ResourceLocation> copied = new HashSet<>();
        for (Entry e : entries) {
            if (!drawable(e, trees) || copied.contains(e.texture) || e.texture.getPath().contains("..")) {
                continue;
            }
            Optional<Resource> resource = resources.getResource(e.texture);
            if (resource.isEmpty()) {
                continue;
            }
            Path target = dir.resolve(e.texture.getNamespace()).resolve(e.texture.getPath());
            Files.createDirectories(target.getParent());
            try (InputStream in = resource.get().open()) {
                Files.copy(in, target, StandardCopyOption.REPLACE_EXISTING);
                copied.add(e.texture);
            } catch (IOException error) {
                HorizonModelDump.LOG.warn("horizon-model-dump: texture {} of {}: {}", e.texture, e.id, error.toString());
            }
        }
        return copied.size();
    }

    private static void writeJson(Path out, List<Entry> entries, Map<String, String> trees) throws IOException {
        Path absolute = out.toAbsolutePath();
        Files.createDirectories(absolute.getParent());
        Path temporary = absolute.resolveSibling(absolute.getFileName() + ".tmp");
        try (Writer file = Files.newBufferedWriter(temporary, StandardCharsets.UTF_8);
                JsonWriter w = new JsonWriter(file)) {
            w.beginObject();
            w.name("format").value(1);
            w.name("entities").beginObject();
            for (Entry e : entries) {
                entity(w, e, trees);
            }
            w.endObject();
            w.name("models").beginObject();
            for (Map.Entry<String, String> tree : trees.entrySet()) {
                w.name(tree.getKey()).jsonValue(tree.getValue());
            }
            w.endObject();
            w.endObject();
        }
        try {
            Files.move(temporary, absolute, StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        } catch (AtomicMoveNotSupportedException error) {
            Files.move(temporary, absolute, StandardCopyOption.REPLACE_EXISTING);
        }
    }

    private static void entity(JsonWriter w, Entry e, Map<String, String> trees) throws IOException {
        w.name(e.id.toString()).beginObject();
        if (e.renderer != null) {
            w.name("renderer").value(e.renderer);
        }
        w.name("system").value(e.system);
        w.name("living").value(e.living);
        if (e.layer != null && trees.containsKey(e.layer)) {
            w.name("layer").value(e.layer);
        }
        if (e.texture != null) {
            w.name("texture").value(e.texture.toString());
        }
        w.name("width");
        PartJson.number(w, e.width);
        w.name("height");
        PartJson.number(w, e.height);
        w.name("eye_height");
        PartJson.number(w, e.eyeHeight);
        Map<String, String> clips = new TreeMap<>();
        for (Map.Entry<String, Object> clip : e.animations.entrySet()) {
            String json = AnimationJson.json(clip.getValue());
            if (json != null) {
                clips.put(clip.getKey(), json);
            }
        }
        if (!clips.isEmpty()) {
            w.name("animations").beginObject();
            for (Map.Entry<String, String> clip : clips.entrySet()) {
                w.name(clip.getKey()).jsonValue(clip.getValue());
            }
            w.endObject();
        }
        if (e.error != null) {
            w.name("error").value(e.error);
        }
        w.endObject();
    }

    private static String summary(List<Entry> entries, Map<String, String> trees, int textures) {
        Map<String, Integer> systems = new TreeMap<>();
        int drawable = 0;
        int animated = 0;
        for (Entry e : entries) {
            systems.merge(e.system, 1, Integer::sum);
            if (drawable(e, trees)) {
                drawable++;
                if (!e.animations.isEmpty()) {
                    animated++;
                }
            }
        }
        return entries.size() + " modded entity types " + systems + ", " + drawable
            + " living with a model and texture (" + animated + " with keyframe clips), " + trees.size()
            + " model trees, " + textures + " textures copied";
    }
}
