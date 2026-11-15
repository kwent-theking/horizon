package dev.horizonlab.modeldump;

import com.google.gson.stream.JsonWriter;
import java.io.IOException;
import java.io.StringWriter;
import java.lang.reflect.Field;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.HashSet;
import java.util.IdentityHashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import net.minecraft.client.model.geom.ModelPart;
import org.joml.Vector3f;

/**
 * A model's part tree as one entry of Horizon's {@code entity-models.json}: {@code pose} (x, y, z,
 * xRot, yRot, zRot, xScale, yScale, zScale), {@code polygons} (four vertices of x, y, z in model
 * pixels and u, v in texture fractions), {@code normals} (one per polygon) and named
 * {@code children}, like mods/horizon's VanillaEntityModelExporter. Reads vanilla {@link ModelPart}s
 * and the Citadel-style box trees of LionfishAPI (Cataclysm): boxes with {@code rotationPoint*},
 * {@code rotateAngle*}, {@code *Scale}, {@code cubeList} (quads of {@code vertexPositions} with
 * {@code position}, {@code textureU}, {@code textureV} and a {@code normal}) and
 * {@code childModels}, posed in vanilla's order (translate, Z·Y·X rotation, scale; vertices / 16).
 */
final class PartJson {
    /** Parts a model holds in its fields, under those fields' names, for a model with no single root. */
    record Fields(List<Map.Entry<String, ModelPart>> parts) {}

    private static final Field CUBES = declared(ModelPart.class, "cubes");
    private static final Field CHILDREN = declared(ModelPart.class, "children");
    private static final Field POLYGONS = declared(ModelPart.Cube.class, "polygons");
    private static final Class<?> POLYGON = nested("Polygon");
    private static final Class<?> VERTEX = nested("Vertex");
    private static final Field VERTICES = declared(POLYGON, "vertices");
    private static final Field NORMAL = declared(POLYGON, "normal");
    private static final Field POS = declared(VERTEX, "pos");
    private static final Field U = declared(VERTEX, "u");
    private static final Field V = declared(VERTEX, "v");

    private PartJson() {}

    /** The JSON of {@code root}: a {@link ModelPart}, a {@link Fields} or a box. */
    static String json(Object root, Map<Object, String> boxNames) throws IOException, ReflectiveOperationException {
        StringWriter out = new StringWriter();
        try (JsonWriter w = new JsonWriter(out)) {
            if (root instanceof ModelPart part) {
                modelPart(w, part);
            } else if (root instanceof Fields fields) {
                w.beginObject();
                pose(w, 0, 0, 0, 0, 0, 0, 1, 1, 1);
                w.name("polygons").beginArray().endArray();
                w.name("normals").beginArray().endArray();
                w.name("children").beginObject();
                for (Map.Entry<String, ModelPart> entry : fields.parts()) {
                    w.name(entry.getKey());
                    modelPart(w, entry.getValue());
                }
                w.endObject();
                w.endObject();
            } else {
                box(w, root, boxNames, Reflect.identitySet());
            }
        }
        return out.toString();
    }

    @SuppressWarnings("unchecked")
    private static void modelPart(JsonWriter w, ModelPart part) throws IOException, ReflectiveOperationException {
        w.beginObject();
        pose(w, part.x, part.y, part.z, part.xRot, part.yRot, part.zRot, part.xScale, part.yScale, part.zScale);
        List<Object> polygons = new ArrayList<>();
        for (Object cube : (List<Object>) CUBES.get(part)) {
            for (Object polygon : (Object[]) POLYGONS.get(cube)) {
                polygons.add(polygon);
            }
        }
        w.name("polygons").beginArray();
        for (Object polygon : polygons) {
            w.beginArray();
            for (Object vertex : (Object[]) VERTICES.get(polygon)) {
                Vector3f pos = (Vector3f) POS.get(vertex);
                vertex(w, pos, U.getFloat(vertex), V.getFloat(vertex));
            }
            w.endArray();
        }
        w.endArray();
        w.name("normals").beginArray();
        for (Object polygon : polygons) {
            vector(w, (Vector3f) NORMAL.get(polygon));
        }
        w.endArray();
        w.name("children").beginObject();
        for (Map.Entry<String, ModelPart> child : ((Map<String, ModelPart>) CHILDREN.get(part)).entrySet()) {
            w.name(child.getKey());
            modelPart(w, child.getValue());
        }
        w.endObject();
        w.endObject();
    }

    /** Whether {@code value} is a Citadel-style box (it has cubes and children lists). */
    static boolean isBox(Object value) {
        return !Reflect.fields(value.getClass(), "cubeList").isEmpty()
            && !Reflect.fields(value.getClass(), "childModels").isEmpty();
    }

    /** The model's field names of the boxes it holds, to name boxes that have no {@code boxName}. */
    static Map<Object, String> fieldNames(Object model) {
        Map<Object, String> names = new IdentityHashMap<>();
        for (Class<?> c = model.getClass(); c != null && c != Object.class; c = c.getSuperclass()) {
            for (Field f : c.getDeclaredFields()) {
                if (Modifier.isStatic(f.getModifiers())) {
                    continue;
                }
                try {
                    f.setAccessible(true);
                    Object value = f.get(model);
                    if (value != null && !(value instanceof ModelPart) && isBox(value)) {
                        names.putIfAbsent(value, f.getName());
                    }
                } catch (ReflectiveOperationException | RuntimeException error) {
                    // Not readable: that box keeps a generated name.
                }
            }
        }
        return names;
    }

    private static void box(JsonWriter w, Object box, Map<Object, String> names, Set<Object> seen)
            throws IOException {
        seen.add(box);
        w.beginObject();
        pose(w, Reflect.getFloat(box, 0, "rotationPointX"), Reflect.getFloat(box, 0, "rotationPointY"),
            Reflect.getFloat(box, 0, "rotationPointZ"), Reflect.getFloat(box, 0, "rotateAngleX"),
            Reflect.getFloat(box, 0, "rotateAngleY"), Reflect.getFloat(box, 0, "rotateAngleZ"),
            Reflect.getFloat(box, 1, "xScale", "scaleX"), Reflect.getFloat(box, 1, "yScale", "scaleY"),
            Reflect.getFloat(box, 1, "zScale", "scaleZ"));
        List<Object> quads = new ArrayList<>();
        for (Object cube : all(box, "cubeList")) {
            if (Reflect.get(cube, "quads") instanceof Object[] array) {
                for (Object quad : array) {
                    if (quad != null && Reflect.get(quad, "vertexPositions") instanceof Object[] vertices
                            && vertices.length == 4 && Reflect.get(quad, "normal") instanceof Vector3f) {
                        quads.add(quad);
                    }
                }
            }
        }
        w.name("polygons").beginArray();
        for (Object quad : quads) {
            w.beginArray();
            for (Object vertex : (Object[]) Reflect.get(quad, "vertexPositions")) {
                Vector3f position = Reflect.get(vertex, "position") instanceof Vector3f p ? p : new Vector3f();
                vertex(w, position, Reflect.getFloat(vertex, 0, "textureU"), Reflect.getFloat(vertex, 0, "textureV"));
            }
            w.endArray();
        }
        w.endArray();
        w.name("normals").beginArray();
        for (Object quad : quads) {
            vector(w, (Vector3f) Reflect.get(quad, "normal"));
        }
        w.endArray();
        w.name("children").beginObject();
        Set<String> used = new HashSet<>();
        int index = 0;
        for (Object child : all(box, "childModels")) {
            index++;
            if (seen.contains(child) || Boolean.FALSE.equals(Reflect.get(child, "showModel"))) {
                continue;
            }
            w.name(unique(boxName(child, names, index), used));
            box(w, child, names, seen);
        }
        w.endObject();
        w.endObject();
    }

    private static String boxName(Object box, Map<Object, String> names, int index) {
        if (Reflect.get(box, "boxName") instanceof String name && !name.isBlank()) {
            return name;
        }
        String field = names.get(box);
        return field != null ? field : "part" + index;
    }

    private static String unique(String name, Set<String> used) {
        String out = name;
        for (int i = 2; !used.add(out); i++) {
            out = name + "_" + i;
        }
        return out;
    }

    /** The elements of every list field {@code name} up the class chain, each once. */
    private static List<Object> all(Object target, String name) {
        Set<Object> seen = Reflect.identitySet();
        List<Object> out = new ArrayList<>();
        for (Field f : Reflect.fields(target.getClass(), name)) {
            try {
                if (f.get(target) instanceof Iterable<?> items) {
                    for (Object item : items) {
                        if (item != null && seen.add(item)) {
                            out.add(item);
                        }
                    }
                }
            } catch (ReflectiveOperationException | RuntimeException error) {
                // An unreadable list adds nothing.
            }
        }
        return out;
    }

    /** The model's {@link ModelPart} fields (and arrays and lists of them), from its class up. */
    static Map<String, ModelPart> modelParts(Object model) {
        Map<String, ModelPart> out = new LinkedHashMap<>();
        Set<ModelPart> seen = Reflect.identitySet();
        for (Class<?> c = model.getClass(); c != null && c != Object.class; c = c.getSuperclass()) {
            for (Field f : c.getDeclaredFields()) {
                if (Modifier.isStatic(f.getModifiers())) {
                    continue;
                }
                try {
                    f.setAccessible(true);
                    Object value = f.get(model);
                    if (value instanceof ModelPart part && seen.add(part)) {
                        out.putIfAbsent(f.getName(), part);
                    } else if (value instanceof Object[] array) {
                        for (int i = 0; i < array.length; i++) {
                            if (array[i] instanceof ModelPart part && seen.add(part)) {
                                out.putIfAbsent(f.getName() + i, part);
                            }
                        }
                    } else if (value instanceof Iterable<?> items) {
                        int i = 0;
                        for (Object item : items) {
                            if (item instanceof ModelPart part && seen.add(part)) {
                                out.putIfAbsent(f.getName() + i, part);
                            }
                            i++;
                        }
                    }
                } catch (ReflectiveOperationException | RuntimeException error) {
                    // An unreadable field holds no parts we can use.
                }
            }
        }
        return out;
    }

    /** Of {@code parts}, the ones no other of them holds: the tops of a model with no root. */
    static List<Map.Entry<String, ModelPart>> tops(Map<String, ModelPart> parts) {
        Set<ModelPart> nested = Reflect.identitySet();
        for (ModelPart part : parts.values()) {
            part.getAllParts().skip(1).forEach(nested::add);
        }
        List<Map.Entry<String, ModelPart>> out = new ArrayList<>();
        for (Map.Entry<String, ModelPart> entry : parts.entrySet()) {
            if (!nested.contains(entry.getValue())) {
                out.add(entry);
            }
        }
        return out;
    }

    private static void pose(JsonWriter w, float... values) throws IOException {
        w.name("pose").beginArray();
        for (float f : values) {
            number(w, f);
        }
        w.endArray();
    }

    private static void vertex(JsonWriter w, Vector3f pos, float u, float v) throws IOException {
        w.beginArray();
        number(w, pos.x);
        number(w, pos.y);
        number(w, pos.z);
        number(w, u);
        number(w, v);
        w.endArray();
    }

    static void vector(JsonWriter w, Vector3f v) throws IOException {
        w.beginArray();
        number(w, v.x);
        number(w, v.y);
        number(w, v.z);
        w.endArray();
    }

    /** A float as JSON; degenerate (zero-size) faces have NaN UVs, written as 0 like the vanilla exporter. */
    static void number(JsonWriter w, float f) throws IOException {
        w.value(Float.valueOf(Float.isFinite(f) ? f : 0.0F));
    }

    private static Field declared(Class<?> type, String name) {
        try {
            Field f = type.getDeclaredField(name);
            f.setAccessible(true);
            return f;
        } catch (NoSuchFieldException error) {
            throw new IllegalStateException(type.getName() + "." + name + " is missing", error);
        }
    }

    private static Class<?> nested(String simpleName) {
        for (Class<?> c : ModelPart.class.getDeclaredClasses()) {
            if (c.getSimpleName().equals(simpleName)) {
                return c;
            }
        }
        throw new IllegalStateException("ModelPart." + simpleName + " is missing");
    }
}
