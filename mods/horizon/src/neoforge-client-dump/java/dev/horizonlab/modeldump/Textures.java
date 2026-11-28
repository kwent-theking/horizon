package dev.horizonlab.modeldump;

import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import net.minecraft.client.renderer.entity.EntityRenderer;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.packs.resources.ResourceManager;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityType;

/**
 * The texture a renderer draws an entity type with: its {@code getTextureLocation} on a
 * {@link FakeEntity}, else the best of the {@code ResourceLocation} constants of the renderer's and
 * model's classes, else of the texture paths in the renderer's constant pool (a bare path is in the
 * entity's namespace). Only textures the resource manager has count.
 */
final class Textures {
    /** File-name words of the textures drawn over a body rather than the body's own. */
    private static final String[] OVERLAYS = {"glow", "eyes", "emissive", "overlay", "saddle", "armor",
        "layer", "crack", "charge", "_light", "pattern", "collar", "harness"};
    private static final int MAX_ELEMENTS = 64;

    private Textures() {}

    static ResourceLocation of(EntityRenderer<?> renderer, Object model, ResourceLocation id, EntityType<?> type,
            ResourceManager resources) {
        Entity fake = FakeEntity.of(type, renderer.getClass());
        ResourceLocation direct = direct(renderer, fake);
        if (usable(direct, resources)) {
            return direct;
        }
        List<ResourceLocation> candidates = new ArrayList<>();
        for (Class<?> c : Reflect.modClasses(renderer.getClass())) {
            constants(c, candidates);
        }
        if (model != null) {
            for (Class<?> c : Reflect.modClasses(model.getClass())) {
                constants(c, candidates);
            }
        }
        ResourceLocation best = best(candidates, id, resources);
        if (best != null) {
            return best;
        }
        candidates.clear();
        for (Class<?> c : Reflect.modClasses(renderer.getClass())) {
            for (String text : ClassConstants.of(c).strings) {
                ResourceLocation location = location(text, id.getNamespace());
                if (location != null) {
                    candidates.add(location);
                }
            }
        }
        best = best(candidates, id, resources);
        return best != null ? best : direct;
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static ResourceLocation direct(EntityRenderer<?> renderer, Entity fake) {
        try {
            return ((EntityRenderer) renderer).getTextureLocation(fake);
        } catch (Throwable error) {
            return null;
        }
    }

    private static void constants(Class<?> type, List<ResourceLocation> out) {
        Field[] fields;
        try {
            fields = type.getDeclaredFields();
        } catch (RuntimeException | LinkageError error) {
            return;
        }
        for (Field f : fields) {
            if (!Modifier.isStatic(f.getModifiers())) {
                continue;
            }
            try {
                f.setAccessible(true);
                collect(f.get(null), out, 0);
            } catch (ReflectiveOperationException | RuntimeException | LinkageError error) {
                // An unreadable constant names nothing.
            }
        }
    }

    private static void collect(Object value, List<ResourceLocation> out, int depth) {
        if (value instanceof ResourceLocation location) {
            out.add(location);
        } else if (depth > 1 || value == null) {
            return;
        } else if (value instanceof Object[] array) {
            for (int i = 0; i < array.length && i < MAX_ELEMENTS; i++) {
                collect(array[i], out, depth + 1);
            }
        } else if (value instanceof Map<?, ?> map) {
            int n = 0;
            for (Object element : map.values()) {
                if (n++ >= MAX_ELEMENTS) {
                    break;
                }
                collect(element, out, depth + 1);
            }
        } else if (value instanceof Iterable<?> items) {
            int n = 0;
            for (Object element : items) {
                if (n++ >= MAX_ELEMENTS) {
                    break;
                }
                collect(element, out, depth + 1);
            }
        }
    }

    /** A texture path constant ({@code ns:textures/....png} or {@code textures/....png}) as a location. */
    static ResourceLocation location(String text, String namespace) {
        if (!text.endsWith(".png") || !text.contains("textures/")) {
            return null;
        }
        try {
            return text.contains(":") ? ResourceLocation.tryParse(text) : ResourceLocation.fromNamespaceAndPath(namespace, text);
        } catch (RuntimeException error) {
            return null;
        }
    }

    private static ResourceLocation best(List<ResourceLocation> candidates, ResourceLocation id, ResourceManager resources) {
        ResourceLocation best = null;
        int bestScore = Integer.MIN_VALUE;
        for (ResourceLocation candidate : candidates) {
            if (!usable(candidate, resources)) {
                continue;
            }
            int score = score(candidate, id);
            if (score > bestScore) {
                best = candidate;
                bestScore = score;
            }
        }
        return best;
    }

    private static int score(ResourceLocation texture, ResourceLocation id) {
        String path = texture.getPath();
        String file = path.substring(path.lastIndexOf('/') + 1, path.length() - ".png".length()).toLowerCase(Locale.ROOT);
        String entity = id.getPath().toLowerCase(Locale.ROOT);
        int score = 0;
        if (file.equals(entity) || file.equals(entity.replace("_", ""))) {
            score += 4;
        } else if (path.contains(entity)) {
            score += 2;
        }
        if (texture.getNamespace().equals(id.getNamespace())) {
            score += 1;
        }
        if (path.startsWith("textures/entity")) {
            score += 1;
        }
        boolean overlay = file.endsWith("_e");
        for (String word : OVERLAYS) {
            overlay |= file.contains(word);
        }
        return overlay ? score - 3 : score;
    }

    static boolean usable(ResourceLocation texture, ResourceManager resources) {
        return texture != null && texture.getPath().startsWith("textures/") && texture.getPath().endsWith(".png")
            && resources.getResource(texture).isPresent();
    }
}
