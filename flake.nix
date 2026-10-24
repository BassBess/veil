{
  description = "Veil - local-first privacy platform";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          python312
          nodejs_22
          rustc
          cargo
          pkg-config
          git
        ];

        shellHook = ''
          echo ""
          echo "================================"
          echo "    VEIL Development Environment"
          echo "================================"
          echo ""
          echo "Python: $(python3 --version)"
          echo "Node:   $(node --version)"
          echo "Rust:   $(rustc --version)"
          echo ""
        '';
      };
    };
}
