package dev.horizonlab.modeldump;

import com.mojang.logging.LogUtils;
import java.nio.file.Files;
import java.nio.file.Path;
import net.minecraft.client.Minecraft;
import net.neoforged.api.distmarker.Dist;
import net.neoforged.fml.common.Mod;
import net.neoforged.fml.loading.FMLPaths;
import net.neoforged.neoforge.client.event.ClientTickEvent;
import net.neoforged.neoforge.common.NeoForge;
import org.slf4j.Logger;

/**
 * Dev-only: once the client has finished loading its resources (on whatever first screen the
 * modpack shows), writes the model, texture and keyframe clips of every modded entity type to
 * {@code modded-entity-models.json} (see {@link ModelDump}) and the textures it names beside it,
 * then quits. Does nothing when the file already exists, so a later normal launch of the profile
 * plays as usual. Never shipped.
 *
 * <p>System properties: {@code horizon.modeldump.out} (the JSON path; default
 * {@code ~/horizon-test/server-states/modded-entity-models.json} when that folder exists, else
 * {@code <game dir>/horizon-model-dump/}), {@code horizon.modeldump.force} (dump even when the file
 * exists) and {@code horizon.modeldump.keepopen} (don't quit afterwards).
 */
@Mod(value = "horizon_model_dump", dist = Dist.CLIENT)
public final class HorizonModelDump {
    static final Logger LOG = LogUtils.getLogger();
    /** Ticks to wait once loading is over, so the first screen's own setup has run. */
    private static final int SETTLE_TICKS = 40;
    private static final String FILE = "modded-entity-models.json";

    private final Path out;
    private int readyTicks;
    private boolean done;

    public HorizonModelDump() {
        out = output();
        if (Files.exists(out) && !flag("horizon.modeldump.force")) {
            LOG.info("horizon-model-dump: {} exists, nothing to do (-Dhorizon.modeldump.force=true redoes it)", out);
            done = true;
            return;
        }
        LOG.info("horizon-model-dump: will write {} once the game has loaded", out);
        NeoForge.EVENT_BUS.addListener(ClientTickEvent.Post.class, e -> tick());
    }

    private void tick() {
        if (done) {
            return;
        }
        Minecraft mc = Minecraft.getInstance();
        if (!mc.isGameLoadFinished() || mc.getOverlay() != null || mc.level != null) {
            readyTicks = 0;
            return;
        }
        if (++readyTicks < SETTLE_TICKS) {
            return;
        }
        done = true;
        try {
            String summary = new ModelDump(mc).write(out);
            LOG.info("horizon-model-dump: done: {}; wrote {}", summary, out);
        } catch (Throwable error) {
            LOG.error("horizon-model-dump: failed, {} not written", out, error);
        }
        if (!flag("horizon.modeldump.keepopen")) {
            mc.stop();
        }
    }

    private static boolean flag(String name) {
        String value = System.getProperty(name);
        return value != null && (value.equals("1") || value.equalsIgnoreCase("true"));
    }

    private static Path output() {
        String explicit = System.getProperty("horizon.modeldump.out");
        if (explicit != null && !explicit.isBlank()) {
            return Path.of(explicit);
        }
        Path states = Path.of(System.getProperty("user.home"), "horizon-test", "server-states");
        if (Files.isDirectory(states)) {
            return states.resolve(FILE);
        }
        // The mod is built before Minecraft is: FML's game folder.
        return FMLPaths.GAMEDIR.get().resolve("horizon-model-dump").resolve(FILE);
    }
}
