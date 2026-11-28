package dev.horizonlab.modeldump;

import java.io.BufferedInputStream;
import java.io.DataInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.util.ArrayList;
import java.util.List;

/**
 * The string constants and field references in a loaded class's constant pool (JVMS 4.4), read
 * from its class file: texture paths a renderer builds its ResourceLocations from, and the static
 * animation fields a model's setupAnim reads.
 */
final class ClassConstants {
    record FieldRef(String owner, String name, String descriptor) {}

    final List<String> strings = new ArrayList<>();
    final List<FieldRef> fields = new ArrayList<>();

    private ClassConstants() {}

    static ClassConstants of(Class<?> type) {
        ClassConstants out = new ClassConstants();
        String resource = type.getName().replace('.', '/') + ".class";
        try (InputStream in = open(type, resource)) {
            if (in != null) {
                out.read(new DataInputStream(new BufferedInputStream(in)));
            }
        } catch (IOException | RuntimeException error) {
            HorizonModelDump.LOG.debug("horizon-model-dump: no constants for {}: {}", type.getName(), error.toString());
        }
        return out;
    }

    private static InputStream open(Class<?> type, String resource) {
        InputStream in = type.getResourceAsStream("/" + resource);
        if (in == null && type.getClassLoader() != null) {
            in = type.getClassLoader().getResourceAsStream(resource);
        }
        return in;
    }

    private void read(DataInputStream in) throws IOException {
        if (in.readInt() != 0xCAFEBABE) {
            throw new IOException("not a class file");
        }
        in.readUnsignedShort();
        in.readUnsignedShort();
        int count = in.readUnsignedShort();
        int[] tags = new int[count];
        String[] utf = new String[count];
        int[] first = new int[count];
        int[] second = new int[count];
        for (int i = 1; i < count; i++) {
            int tag = in.readUnsignedByte();
            tags[i] = tag;
            switch (tag) {
                case 1 -> utf[i] = in.readUTF();
                case 3, 4 -> in.readInt();
                case 5, 6 -> {
                    in.readLong();
                    i++;
                }
                case 7, 8, 16, 19, 20 -> first[i] = in.readUnsignedShort();
                case 9, 10, 11, 12, 17, 18 -> {
                    first[i] = in.readUnsignedShort();
                    second[i] = in.readUnsignedShort();
                }
                case 15 -> {
                    in.readUnsignedByte();
                    first[i] = in.readUnsignedShort();
                }
                default -> throw new IOException("unknown constant tag " + tag);
            }
        }
        for (int i = 1; i < count; i++) {
            if (tags[i] == 8 && utf[first[i]] != null) {
                strings.add(utf[first[i]]);
            } else if (tags[i] == 9) {
                String owner = utf[first[first[i]]];
                int nameAndType = second[i];
                String name = utf[first[nameAndType]];
                String descriptor = utf[second[nameAndType]];
                if (owner != null && name != null && descriptor != null) {
                    fields.add(new FieldRef(owner.replace('/', '.'), name, descriptor));
                }
            }
        }
    }
}
