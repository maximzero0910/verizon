package fixture.architectury;

/** Live assertion for the Fabric attachment accessor implemented by Mixin. */
public final class AttachmentAccessorProbe {
    private AttachmentAccessorProbe() {
    }

    public static void onInitialize() {
        System.err.println("J2H_COMPAT_PROBE_ENTERED:attachment_accessor_probe");
        try {
            Class<?> accessor = Class.forName(
                    "net.fabricmc.fabric.mixin.attachment.ClientboundCustomPayloadPacketAccessor");
            int size = (Integer) accessor.getMethod("getMaxPayloadSize").invoke(null);
            if (size != 1_048_576) {
                throw new IllegalStateException("unexpected clientbound custom payload limit: " + size);
            }
            System.err.println("J2H_COMPAT_ASSERTION:attachment_payload_limit:1048576");
        } catch (ReflectiveOperationException e) {
            throw new IllegalStateException("Fabric attachment payload accessor was not transformed", e);
        }
    }
}
