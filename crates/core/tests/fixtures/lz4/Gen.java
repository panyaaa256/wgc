import java.io.*;
import net.jpountz.lz4.LZ4BlockOutputStream;

public class Gen {
    // Same generators as crates/core/src/region/compression.rs tests.
    static byte[] compressible(int n) {
        byte[] b = new byte[n];
        for (int i = 0; i < n; i++) b[i] = (byte) ((i % 251) ^ ((i / 1000) % 7));
        return b;
    }
    static byte[] noise(int n) {
        byte[] b = new byte[n];
        long s = 0x2545F4914F6CDD1DL;
        for (int i = 0; i < n; i++) { s = s * 6364136223846793005L + 1442695040888963407L; b[i] = (byte) (s >>> 56); }
        return b;
    }
    static void write(String path, byte[] data) throws IOException {
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        try (LZ4BlockOutputStream out = new LZ4BlockOutputStream(bos)) { out.write(data); }
        try (FileOutputStream f = new FileOutputStream(path)) { f.write(bos.toByteArray()); }
    }
    public static void main(String[] a) throws IOException {
        write(a[0] + "/compressible_100000.lz4java", compressible(100000));
        write(a[0] + "/noise_1000.lz4java", noise(1000));
    }
}
