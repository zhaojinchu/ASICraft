// The end of every client tick: the camera mod moves the spectator then.
package dev.redfab.camera.mc.mixin;

import dev.redfab.camera.mc.CameraMod;
import net.minecraft.client.Minecraft;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(Minecraft.class)
public abstract class MinecraftMixin {
    @Inject(method = "tick()V", at = @At("TAIL"))
    private void redfab$tickDone(CallbackInfo ci) {
        CameraMod.tick((Minecraft) (Object) this);
    }
}
