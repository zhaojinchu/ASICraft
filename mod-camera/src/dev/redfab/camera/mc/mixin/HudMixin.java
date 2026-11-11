// No HUD while the camera runs: the stream is the game's picture alone, with no
// crosshair, chat or hotbar over the circuits (what F1 does, without touching
// the game's own state). The page says which plot is on camera and who drives.
package dev.redfab.camera.mc.mixin;

import dev.redfab.camera.mc.CameraMod;
import net.minecraft.client.DeltaTracker;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.Hud;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(Hud.class)
public abstract class HudMixin {
    @Inject(method = "extractRenderState(Lnet/minecraft/client/gui/GuiGraphicsExtractor;Lnet/minecraft/client/DeltaTracker;)V", at = @At("HEAD"), cancellable = true)
    private void redfab$hide(GuiGraphicsExtractor graphics, DeltaTracker delta, CallbackInfo ci) {
        if (CameraMod.hidesHud()) ci.cancel();
    }
}
