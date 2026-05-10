{...}: {
  perSystem = {
    pkgs,
    system,
    lib,
    ...
  }:
    lib.optionalAttrs (system == "x86_64-linux") {
      checks = {
        mkFleet-eval-tests = let
          harness = import ../../tests/lib/mk-fleet {inherit lib;};
          results = harness.results;
          allOk = lib.all (r: r == "ok") results;
        in
          pkgs.runCommand "mkFleet-eval-tests" {} (
            if allOk
            then ''
              echo "PASS: mkFleet harness - ${toString (builtins.length results)} fixtures ok"
              printf '%s\n' ${lib.concatMapStringsSep " " (r: ''"${r}"'') results} > $out
            ''
            else ''
              echo "FAIL: mkFleet harness produced non-ok results: ${builtins.toJSON results}" >&2
              exit 1
            ''
          );
      };
    };
}
