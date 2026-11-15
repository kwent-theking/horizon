package dev.horizonlab.modeldump;

import java.util.ArrayList;
import java.util.List;
import net.minecraft.client.model.geom.EntityModelSet;
import net.minecraft.client.model.geom.ModelLayerLocation;
import net.minecraft.client.model.geom.ModelPart;

/**
 * The game's model set that also notes every layer a renderer bakes, with the part tree it got:
 * a renderer's model holds those very parts, so the layer it was built from is the one whose tree
 * holds them (a baked part doesn't know its layer).
 */
final class RecordingModelSet extends EntityModelSet {
    record Baked(ModelLayerLocation location, ModelPart root) {}

    private final EntityModelSet game;
    private final List<Baked> baked = new ArrayList<>();

    RecordingModelSet(EntityModelSet game) {
        this.game = game;
    }

    @Override
    public ModelPart bakeLayer(ModelLayerLocation location) {
        ModelPart root = game.bakeLayer(location);
        baked.add(new Baked(location, root));
        return root;
    }

    /** The layers baked since the last call, in order. */
    List<Baked> take() {
        List<Baked> out = List.copyOf(baked);
        baked.clear();
        return out;
    }
}
