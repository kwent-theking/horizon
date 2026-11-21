package dev.horizonlab.modeldump;

import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.ArrayList;
import java.util.Collections;
import java.util.HashMap;
import java.util.IdentityHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

/** Field and method access by name, for the classes of other mods this dump only knows by shape. */
final class Reflect {
    private static final Map<Class<?>, Map<String, List<Field>>> FIELDS = new HashMap<>();

    private Reflect() {}

    /** Every field named {@code name} from {@code type} up, nearest first (a subclass may shadow its parent's). */
    static List<Field> fields(Class<?> type, String name) {
        return FIELDS.computeIfAbsent(type, t -> new HashMap<>()).computeIfAbsent(name, n -> {
            List<Field> out = new ArrayList<>();
            for (Class<?> c = type; c != null && c != Object.class; c = c.getSuperclass()) {
                for (Field f : c.getDeclaredFields()) {
                    if (f.getName().equals(n) && !Modifier.isStatic(f.getModifiers()) && accessible(f)) {
                        out.add(f);
                    }
                }
            }
            return out;
        });
    }

    /** The nearest instance field {@code name} of {@code target}, or null. */
    static Object get(Object target, String name) {
        for (Field f : fields(target.getClass(), name)) {
            try {
                return f.get(target);
            } catch (ReflectiveOperationException | RuntimeException error) {
                return null;
            }
        }
        return null;
    }

    /** The first of the float fields {@code names} that {@code target} has, else {@code fallback}. */
    static float getFloat(Object target, float fallback, String... names) {
        for (String name : names) {
            if (get(target, name) instanceof Float value) {
                return value;
            }
        }
        return fallback;
    }

    /** {@code target.name()} (a public no-argument method), or null when it fails or is missing. */
    static Object call(Object target, String name) {
        try {
            Method method = target.getClass().getMethod(name);
            return method.getParameterCount() == 0 ? method.invoke(target) : null;
        } catch (ReflectiveOperationException | RuntimeException | LinkageError error) {
            return null;
        }
    }

    static Object getStatic(Class<?> type, String name) throws ReflectiveOperationException {
        Field f = type.getDeclaredField(name);
        f.setAccessible(true);
        return f.get(null);
    }

    static boolean set(Object target, Class<?> owner, String name, Object value) {
        try {
            Field f = owner.getDeclaredField(name);
            f.setAccessible(true);
            f.set(target, value);
            return true;
        } catch (ReflectiveOperationException | RuntimeException error) {
            return false;
        }
    }

    /** {@code type} and its superclasses below {@code net.minecraft} and the JDK: a mod's own classes. */
    static List<Class<?>> modClasses(Class<?> type) {
        List<Class<?>> out = new ArrayList<>();
        for (Class<?> c = type; c != null && !c.getName().startsWith("net.minecraft.")
                && !c.getName().startsWith("java."); c = c.getSuperclass()) {
            out.add(c);
        }
        return out;
    }

    static boolean hierarchyStartsWith(Class<?> type, String prefix) {
        for (Class<?> c = type; c != null; c = c.getSuperclass()) {
            if (c.getName().startsWith(prefix)) {
                return true;
            }
        }
        return false;
    }

    static <T> Set<T> identitySet() {
        return Collections.newSetFromMap(new IdentityHashMap<>());
    }

    private static boolean accessible(Field f) {
        try {
            f.setAccessible(true);
            return true;
        } catch (RuntimeException error) {
            return false;
        }
    }
}
