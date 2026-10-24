# The AI front end's origin vhosts. The **names** are rows in `common/front-door.nix`
# (`ai.int` and `ai.leighhack.org`, the latter with the gate); `mcp` and `whisper` are
# direct rows there, so they have no vhost on this box any more — the door proxies
# straight to aibox.
#
# Both rows here are `upstream = "origin"` because the app is not one line: the chat UI
# serves a `/resources` file root and a PWA manifest that has to stay unauthenticated.
# The gate and the certificate are the door's; what stays is the routing.
{lib, ...}: let
  mkOriginVhost = import ../../common/nginx/origin-vhost.nix {inherit lib;};
in {
  services.nginx.virtualHosts = {
    # The public name. It used to be built with `lib.mkMerge` of an `mkSSOVirtualHost`
    # over an `mkIntVhost`, which is how the front-door table's first draft missed its
    # gate (migration doc, phase 2 lesson 2). The gate is now a `sso = true` on the
    # table row; this vhost only has to keep the paths.
    "ai.leighhack.org" =
      mkOriginVhost {
        proxyPass = "http://10.3.1.32:8081";
      }
      // {
        locations."/resources" = {
          root = "/srv/ai-resources";
        };

        # Browsers fetch the PWA manifest without cookies, so the gate would 401 it and
        # the console would show a CORS error. It is still a proxy to the app (not a
        # static file) because the manifest is generated there.
        locations."= /manifest.webmanifest" = {
          proxyPass = "http://10.3.1.32:8081";
          recommendedProxySettings = true;
        };
      };

    # Long timeouts and a big body limit: the chat UI uploads images and can hold a
    # request open while the model thinks.
    "ai.int.leighhack.org" = mkOriginVhost {
      proxyPass = "http://10.3.1.32:8081";
      bodySize = "1024M";
      timeouts = "3600";
    };

    # "sd.ai.leighhack.org" = {
    #   useACMEHost = "leighhack.org";
    #   forceSSL = true;

    #   locations."/" = {
    #     proxyPass = "http://10.3.1.32:7860";
    #     recommendedProxySettings = true;
    #     proxyWebsockets = true;
    #     basicAuthFile = CONFIG.HTTP_BASIC_AUTH_FILE;
    #   };
    # };
  };
}
