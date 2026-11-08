# Builds the three VapourSynth plugins that have no Linux wheel (ffms2, fmtconv,
# miscfilters) from pinned source on Ubuntu 24.04. Used by scripts/build-vs-plugins.sh,
# which copies the results out and proves they load under VapourSynth R81.
ARG UBUNTU=26.04
FROM ubuntu:${UBUNTU}
ARG VS_TAG=R81
ARG FFMS2_TAG=5.0
ARG FMTCONV_TAG=r30
ARG MISC_TAG=R2
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential git curl ca-certificates pkg-config autoconf automake libtool \
      meson ninja-build python3 \
      libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# Headers only: plugins are API 4, so the R81 headers are all they need.
RUN git clone --depth 1 --branch "$VS_TAG" https://github.com/vapoursynth/vapoursynth.git \
 && git clone --depth 1 --branch "$FFMS2_TAG" https://github.com/FFMS/ffms2.git \
 && git clone --depth 1 --branch "$FMTCONV_TAG" https://github.com/EleonoreMizo/fmtconv.git \
 && git clone --depth 1 --branch "$MISC_TAG" https://github.com/vapoursynth/vs-miscfilters-obsolete.git \
 && for d in vapoursynth ffms2 fmtconv vs-miscfilters-obsolete; do \
      echo "$d $(git -C $d rev-parse HEAD)"; done > /src/REVISIONS
RUN mkdir -p /usr/local/include/vapoursynth \
 && cp vapoursynth/include/*.h /usr/local/include/vapoursynth/ \
 && printf 'prefix=/usr/local\nincludedir=${prefix}/include\nName: vapoursynth\nDescription: headers\nVersion: 81\nCflags: -I${includedir}/vapoursynth\n' > /usr/lib/pkgconfig/vapoursynth.pc \
 && cp /usr/lib/pkgconfig/vapoursynth.pc /usr/lib/pkgconfig/vapoursynth-script.pc
RUN cd ffms2 && ./autogen.sh && ./configure --enable-shared --disable-static && make -j"$(nproc)"
RUN cd fmtconv/build/unix && ./autogen.sh >/dev/null && ./configure >/dev/null && make -j"$(nproc)" >/dev/null
# The repository has no build system (only src/ and an MSVC project); the single source
# needs filtershared.h from the VapourSynth core tree. R81's copy uses an API 4.3 call
# (getNodeResidency) that the R81 header does not declare, so take the R73 copy.
RUN mkdir -p /src/misc-build /src/shared \
 && curl -fsSL https://raw.githubusercontent.com/vapoursynth/vapoursynth/R73/src/core/filtershared.h -o /src/shared/filtershared.h \
 && g++ -std=c++17 -O2 -fPIC -shared -msse2 \
      -I/usr/local/include/vapoursynth -I/src/shared \
      vs-miscfilters-obsolete/src/miscfilters.cpp -o /src/misc-build/libmiscfilters.so
RUN mkdir -p /out && cp /src/REVISIONS /out/ \
 && find ffms2 fmtconv misc-build \( -name 'libffms2.so*' -o -name 'libfmtconv.so' -o -name 'libmiscfilters.so' \) -exec cp -P {} /out/ \; \
 && ls -la /out
