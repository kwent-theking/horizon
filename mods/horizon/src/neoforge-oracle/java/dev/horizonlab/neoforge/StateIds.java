package dev.horizonlab.neoforge;

import java.io.BufferedWriter;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.core.Registry;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;

/**
 * The network ids of a running server ({@code -Dhorizon.oracle.stateids=FILE}): every block state
 * id with its block and properties ({@code Block.BLOCK_STATE_REGISTRY}, which NeoForge rebuilds
 * from the frozen block registry, so modded states included), and the id-to-name tables of the
 * registries a client needs to read the server's packets. A client joining that server maps the
 * chunk palettes and item stacks with them. Dev only, never shipped.
 */
final class StateIds {
    private StateIds() {}

    static void write(Path out) throws IOException {
        Path dir = out.toAbsolutePath().getParent();
        if (dir != null) {
            Files.createDirectories(dir);
        }
        try (BufferedWriter w = Files.newBufferedWriter(out, StandardCharsets.UTF_8)) {
            int size = Block.BLOCK_STATE_REGISTRY.size();
            w.write("# horizon-oracle state ids v1\t" + size + "\n");
            for (int id = 0; id < size; id++) {
                BlockState state = Block.BLOCK_STATE_REGISTRY.byId(id);
                if (state == null) {
                    w.write(id + "\t\t\n");
                    continue;
                }
                w.write(id + "\t" + BuiltInRegistries.BLOCK.getKey(state.getBlock()) + "\t" + key(state) + "\n");
            }
        }
        registry(sibling(out, "blocks"), BuiltInRegistries.BLOCK);
        registry(sibling(out, "items"), BuiltInRegistries.ITEM);
        registry(sibling(out, "entities"), BuiltInRegistries.ENTITY_TYPE);
        registry(sibling(out, "block_entities"), BuiltInRegistries.BLOCK_ENTITY_TYPE);
        registry(sibling(out, "menus"), BuiltInRegistries.MENU);
        try (BufferedWriter w = Files.newBufferedWriter(sibling(out, "registries"), StandardCharsets.UTF_8)) {
            for (Registry<?> registry : BuiltInRegistries.REGISTRY) {
                entries(w, registry.key().location().toString(), registry);
            }
        }
    }

    /** Every entry of {@code registry} as {@code registry TAB id TAB name}. */
    private static <T> void entries(BufferedWriter w, String name, Registry<T> registry) throws IOException {
        for (T value : registry) {
            w.write(name + "\t" + registry.getId(value) + "\t" + registry.getKey(value) + "\n");
        }
    }

    static Path sibling(Path out, String what) {
        String name = out.getFileName().toString();
        int dot = name.lastIndexOf('.');
        String stem = dot > 0 ? name.substring(0, dot) : name;
        return out.resolveSibling(stem + "." + what + ".tsv");
    }

    private static <T> void registry(Path out, Registry<T> registry) throws IOException {
        try (BufferedWriter w = Files.newBufferedWriter(out, StandardCharsets.UTF_8)) {
            for (T value : registry) {
                w.write(registry.getId(value) + "\t" + registry.getKey(value) + "\n");
            }
        }
    }

    private static String key(BlockState state) {
        StringBuilder b = new StringBuilder();
        for (Property<?> p : state.getProperties()) {
            if (b.length() > 0) {
                b.append(',');
            }
            b.append(p.getName()).append('=').append(name(p, state.getValue(p)));
        }
        return b.toString();
    }

    @SuppressWarnings({"unchecked", "rawtypes"})
    private static String name(Property p, Object v) {
        return p.getName((Comparable) v);
    }
}
