// One line in the corner of the HUD, after the game's own: the plot the camera
// is on and who drives it, so the stream says so too.
package dev.redfab.camera.mc.mixin;

import dev.redfab.camera.mc.CameraMod;
import net.minecraft.client.DeltaTracker;
import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.GuiGraphicsExtractor;
import net.minecraft.client.gui.Hud;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(Hud.class)
public abstract class HudMixin {
    @Inject(method = "extractRenderState(Lnet/minecraft/client/gui/GuiGraphicsExtractor;Lnet/minecraft/client/DeltaTracker;)V", at = @At("TAIL"))
    private void redfab$overlay(GuiGraphicsExtractor graphics, DeltaTracker delta, CallbackInfo ci) {
        String line = CameraMod.overlay();
        if (line != null) graphics.text(Minecraft.getInstance().font, line, 4, 4, 0xFFFFFFFF);
    }
}
