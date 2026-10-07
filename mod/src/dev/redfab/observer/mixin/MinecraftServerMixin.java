// The end of every server tick: the changes collected during it go out with
// the overworld's game time, which `/time query gametime` also reports. The
// start of a tick flushes what changed between ticks (commands run there, while
// the clock still reads the last completed tick) under that tick's number, so a
// command's changes are never mixed with the next tick's.
package dev.redfab.observer.mixin;

import java.util.function.BooleanSupplier;

import dev.redfab.observer.Observer;
import net.minecraft.server.MinecraftServer;
import org.spongepowered.asm.mixin.Mixin;
import org.spongepowered.asm.mixin.injection.At;
import org.spongepowered.asm.mixin.injection.Inject;
import org.spongepowered.asm.mixin.injection.callback.CallbackInfo;

@Mixin(MinecraftServer.class)
public abstract class MinecraftServerMixin {
    @Inject(method = "tickServer(Ljava/util/function/BooleanSupplier;)V", at = @At("HEAD"))
    private void redfab$tickStarts(BooleanSupplier hasTimeLeft, CallbackInfo ci) {
        MinecraftServer server = (MinecraftServer) (Object) this;
        Observer.betweenTicks(server.overworld().getGameTime());
    }

    @Inject(method = "tickServer(Ljava/util/function/BooleanSupplier;)V", at = @At("TAIL"))
    private void redfab$tickDone(BooleanSupplier hasTimeLeft, CallbackInfo ci) {
        MinecraftServer server = (MinecraftServer) (Object) this;
        Observer.tickDone(server.overworld().getGameTime());
    }
}
