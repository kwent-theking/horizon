package dev.horizonlab.modeldump;

import java.lang.reflect.Field;
import java.lang.reflect.GenericArrayType;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.ParameterizedType;
import java.lang.reflect.Type;
import java.lang.reflect.TypeVariable;
import java.lang.reflect.WildcardType;
import java.util.HashMap;
import java.util.Map;
import net.minecraft.client.renderer.entity.EntityRenderer;
import net.minecraft.core.BlockPos;
import net.minecraft.network.syncher.SynchedEntityData;
import net.minecraft.util.RandomSource;
import net.minecraft.world.entity.Entity;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.phys.Vec3;

/**
 * An entity of the class a renderer draws, made without its constructor (there is no level on the
 * title screen): its type, random, position and default synched data are filled in, which is what
 * most {@code getTextureLocation}s read (a variant, a baby flag). Null when the class is abstract or
 * can't be made.
 */
final class FakeEntity {
    private FakeEntity() {}

    static Entity of(EntityType<?> type, Class<?> rendererClass) {
        Class<?> entityClass = entityClass(rendererClass);
        if (entityClass == null || Modifier.isAbstract(entityClass.getModifiers())
                || !Entity.class.isAssignableFrom(entityClass)) {
            return null;
        }
        Entity entity;
        try {
            Class<?> unsafeClass = Class.forName("sun.misc.Unsafe");
            Field theUnsafe = unsafeClass.getDeclaredField("theUnsafe");
            theUnsafe.setAccessible(true);
            Object unsafe = theUnsafe.get(null);
            Method allocate = unsafeClass.getMethod("allocateInstance", Class.class);
            entity = (Entity) allocate.invoke(unsafe, entityClass);
        } catch (ReflectiveOperationException | RuntimeException | LinkageError error) {
            return null;
        }
        Reflect.set(entity, Entity.class, "type", type);
        Reflect.set(entity, Entity.class, "random", RandomSource.create());
        Reflect.set(entity, Entity.class, "position", Vec3.ZERO);
        Reflect.set(entity, Entity.class, "blockPosition", BlockPos.ZERO);
        Reflect.set(entity, Entity.class, "dimensions", type.getDimensions());
        try {
            SynchedEntityData.Builder builder = new SynchedEntityData.Builder(entity);
            Method define = Entity.class.getDeclaredMethod("defineSynchedData", SynchedEntityData.Builder.class);
            define.setAccessible(true);
            define.invoke(entity, builder);
            Reflect.set(entity, Entity.class, "entityData", builder.build());
        } catch (ReflectiveOperationException | RuntimeException | LinkageError error) {
            // No synched data: a texture that reads it falls back to the renderer's constants.
        }
        return entity;
    }

    /** The {@code T} the renderer class binds {@code EntityRenderer<T>} to, erased. */
    static Class<?> entityClass(Class<?> rendererClass) {
        Map<TypeVariable<?>, Type> bindings = new HashMap<>();
        try {
            for (Class<?> c = rendererClass; c != null && c != Object.class; ) {
                Type parent = c.getGenericSuperclass();
                Class<?> raw = erase(parent, bindings);
                if (raw == null) {
                    return null;
                }
                if (parent instanceof ParameterizedType parameterized) {
                    TypeVariable<?>[] variables = raw.getTypeParameters();
                    Type[] arguments = parameterized.getActualTypeArguments();
                    for (int i = 0; i < variables.length && i < arguments.length; i++) {
                        bindings.put(variables[i], arguments[i]);
                    }
                }
                if (raw == EntityRenderer.class) {
                    return erase(bindings.get(EntityRenderer.class.getTypeParameters()[0]), bindings);
                }
                c = raw;
            }
        } catch (RuntimeException | LinkageError error) {
            return null;
        }
        return null;
    }

    private static Class<?> erase(Type type, Map<TypeVariable<?>, Type> bindings) {
        for (int depth = 0; depth < 16 && type != null; depth++) {
            if (type instanceof Class<?> c) {
                return c;
            }
            if (type instanceof ParameterizedType p) {
                return erase(p.getRawType(), bindings);
            }
            if (type instanceof TypeVariable<?> v) {
                Type bound = bindings.get(v);
                type = bound != null && bound != v ? bound : v.getBounds()[0];
            } else if (type instanceof WildcardType w) {
                type = w.getUpperBounds()[0];
            } else if (type instanceof GenericArrayType) {
                return null;
            } else {
                return null;
            }
        }
        return null;
    }
}
