package dev.horizonlab.neoforge;

import com.google.gson.stream.JsonWriter;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.VoxelShape;

/** Shape samples with a fixed centre state and stone or air at its six face neighbours. */
final class NeighbourShapes implements BlockGetter {
    private static final BlockPos[] SIDES = Arrays.stream(Direction.values())
            .map(BlockPos.ZERO::relative).toArray(BlockPos[]::new);
    private static final int SAMPLES = 1 << SIDES.length;

    private final BlockState centre;
    private final LevelHeightAccessor height;
    private int mask;

    private NeighbourShapes(BlockState centre, LevelHeightAccessor height) {
        this.centre = centre;
        this.height = height;
    }

    @Override
    public BlockState getBlockState(BlockPos pos) {
        if (BlockPos.ZERO.equals(pos)) return centre;
        for (int i = 0; i < SIDES.length; i++) {
            if ((mask & (1 << i)) != 0 && SIDES[i].equals(pos)) {
                return Blocks.STONE.defaultBlockState();
            }
        }
        return Blocks.AIR.defaultBlockState();
    }

    @Override
    public FluidState getFluidState(BlockPos pos) {
        return getBlockState(pos).getFluidState();
    }

    @Override
    public BlockEntity getBlockEntity(BlockPos pos) {
        return null;
    }

    @Override
    public int getHeight() {
        return height.getHeight();
    }

    @Override
    public int getMinBuildHeight() {
        return height.getMinBuildHeight();
    }

    private record Delta(List<AABB> outline, List<AABB> collision) {}

    private static List<AABB> difference(VoxelShape shape, VoxelShape baseline, List<AABB> boxes) {
        if (shape == baseline) return null;
        List<AABB> changed = shape.toAabbs();
        return changed.equals(boxes) ? null : changed;
    }

    static void write(
            JsonWriter json, BlockState state, LevelHeightAccessor height,
            VoxelShape outline, VoxelShape collision, List<AABB> baseOutline, List<AABB> baseCollision)
            throws IOException {
        NeighbourShapes world = new NeighbourShapes(state, height);
        List<Delta> palette = new ArrayList<>();
        int[] samples = new int[SAMPLES];
        Arrays.fill(samples, -1);
        for (int mask = 0; mask < SAMPLES; mask++) {
            world.mask = mask;
            List<AABB> o;
            List<AABB> c;
            try {
                o = difference(state.getShape(world, BlockPos.ZERO), outline, baseOutline);
                c = difference(state.getCollisionShape(world, BlockPos.ZERO), collision, baseCollision);
            } catch (RuntimeException e) {
                // A mod can require a real level or block entity for this case.
                json.name("neighbour_error").value(e.getClass().getName());
                return;
            }
            if (o == null && c == null) continue;
            Delta sample = new Delta(o, c);
            int index = palette.indexOf(sample);
            if (index < 0) {
                index = palette.size();
                palette.add(sample);
            }
            samples[mask] = index;
        }
        if (palette.isEmpty()) return;

        int sides = 0;
        for (int mask = 0; mask < SAMPLES; mask++) {
            for (int bit = 1; bit < SAMPLES; bit <<= 1) {
                if (samples[mask] != samples[mask ^ bit]) sides |= bit;
            }
        }
        json.name("neighbours").beginObject();
        json.name("sides").value(sides);
        json.name("palette").beginArray();
        for (Delta sample : palette) {
            json.beginObject();
            if (sample.outline() != null) boxes(json.name("outline"), sample.outline());
            if (sample.collision() != null) boxes(json.name("collision"), sample.collision());
            json.endObject();
        }
        json.endArray();
        json.name("masks").beginObject();
        for (int mask = 0; mask < SAMPLES; mask++) {
            if ((mask & sides) == mask && samples[mask] >= 0) {
                json.name(Integer.toString(mask)).value(samples[mask]);
            }
        }
        json.endObject();
        json.endObject();
    }

    static void boxes(JsonWriter json, List<AABB> boxes) throws IOException {
        json.beginArray();
        for (AABB b : boxes) {
            json.beginArray();
            json.value(b.minX).value(b.minY).value(b.minZ).value(b.maxX).value(b.maxY).value(b.maxZ);
            json.endArray();
        }
        json.endArray();
    }
}
