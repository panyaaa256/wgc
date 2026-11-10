# LZ4 fixtures

Produced by lz4-java 1.8.0 (`org.lz4:lz4-java`, the library Minecraft uses for
region compression type 4) with `LZ4BlockOutputStream` defaults, by `Gen.java`:

```
javac -cp lz4-java-1.8.0.jar Gen.java
java -cp lz4-java-1.8.0.jar:. Gen .
```

| File | Input |
|---|---|
| `compressible_100000.lz4java` | 100000 bytes, `b[i] = (i % 251) ^ ((i / 1000) % 7)` (two LZ4 blocks) |
| `noise_1000.lz4java` | 1000 bytes of LCG output (stored as a raw block) |

The tests regenerate the inputs with the same formulas.
