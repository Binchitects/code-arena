# Code Arena's UI-test image: the build image with a virtual X server (Xvfb), xdotool to click and type, and Python
# for the screenshot script, so the native window is opened off-screen, driven and seen without anyone's desktop.
#
#   docker build -t code-arena-gui docker/gui-test
#   scripts/ui-test.sh start            # opens target/debug/code-arena ide on a virtual screen
FROM code-arena-builder

RUN apt-get update \
 && apt-get install -y --no-install-recommends xvfb xdotool python3 \
 && rm -rf /var/lib/apt/lists/*

# What the window opens at run time on X11 (winit's cursor, input and keyboard libraries) and a font to draw with.
RUN apt-get update \
 && apt-get install -y --no-install-recommends libxcursor1 libxrandr2 libxi6 libxkbcommon-x11-0 libx11-xcb1 \
      libxcb-shm0 libxcb-randr0 libxcb-xfixes0 fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*

# wgpu makes its drawing surface through EGL even when the window draws in software (FLOEM_FORCE_TINY_SKIA): Mesa's.
RUN apt-get update \
 && apt-get install -y --no-install-recommends libegl1 libegl-mesa0 libgles2 libgbm1 libwayland-client0 \
      libwayland-server0 libwayland-egl1 \
 && rm -rf /var/lib/apt/lists/*
