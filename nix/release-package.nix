{
    stdenv,
    fetchurl,
    appimageTools,
    makeWrapper,
    wrapGAppsHook4,
    autoPatchelfHook,
    glib-networking,
    gsettings-desktop-schemas,
    gtk4,
    webkitgtk_6_0,
    libsoup_3,
    graphene,
    gdk-pixbuf,
    libxkbcommon,
    wayland,
    vulkan-loader,
    libGL,
    libpulseaudio,
    alsa-lib,
    gst_all_1,
    pipewire,
}: let
    pname = "serein";
    version = "1.0.0-nightly.20261007.55";

    src = fetchurl {
        url = "https://github.com{version}/serein-v${version}-Linux-X64.AppImage";
        hash = "sha256-MsbqeHuL1pIJG5Gq8M5NG81WYB4dpbkjp+WjvBsJYAY=";
    };

    extracted = appimageTools.extract {inherit pname version src;};
in
    stdenv.mkDerivation {
        inherit pname version;

        src = extracted;

        nativeBuildInputs = [
            wrapGAppsHook4
            makeWrapper
            autoPatchelfHook
        ];

        buildInputs = [
            stdenv.cc.cc.lib
            webkitgtk_6_0
            gtk4
            gdk-pixbuf
            graphene
            libsoup_3
            glib-networking
            gsettings-desktop-schemas
            libxkbcommon
            wayland
            vulkan-loader
            libGL
            libpulseaudio
            alsa-lib
            gst_all_1.gstreamer
            gst_all_1.gst-plugins-base
            gst_all_1.gst-plugins-good
            pipewire
        ];

        dontDropIconThemeCache = true;

        installPhase = ''
            runHook preInstall

            mkdir -p $out/libexec/${pname}
            cp -r . $out/libexec/${pname}/

            chmod -R +w $out/libexec/${pname}/

            mkdir -p $out/bin
            makeWrapper $out/libexec/${pname}/AppRun $out/bin/${pname}

            install -Dm444 cz.viceverse.serein.desktop $out/share/applications/serein.desktop

            substituteInPlace $out/share/applications/serein.desktop \
              --replace-fail "Exec=serein" "Exec=$out/bin/${pname}"

            cp -r usr/share/icons $out/share/icons

            runHook postInstall
        '';
    }
