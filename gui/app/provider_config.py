from __future__ import annotations

import json
import re
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path


CONFIG_VERSION = 1
CONFIG_FILE = "library-providers.yaml"
EXAMPLE_CONFIG_FILE = "library-providers.example.yaml"


@dataclass(frozen=True)
class Provider:
    id: str
    display_name: str
    prefix: str
    visibility: str
    priority: int
    symbols_path: Path
    footprints_path: Path
    models3d_path: Path
    design_blocks_path: Path = Path("")
    database_path: Path = Path("")
    repo_url: str = ""
    repo_path: Path = Path(".")
    auth_method_last_success: str = ""
    last_verified_at: str = ""
    use_submodule: bool = False
    source: str = "provider"

    def is_reference(self) -> bool:
        return self.source == "kicad"

    def has_parts(self) -> bool:
        return self.source == "provider" and bool(self.prefix.strip())


@dataclass(frozen=True)
class ProviderRegistry:
    providers: list[Provider]
    source_name: str

    def by_id(self, provider_id: str) -> Provider | None:
        for provider in self.providers:
            if provider.id == provider_id:
                return provider
        return None

    def non_reference(self) -> list[Provider]:
        return [provider for provider in self.providers if not provider.is_reference()]

    def writable_providers(self) -> list[Provider]:
        return [provider for provider in self.providers if provider.has_parts()]

    def prefixes(self) -> set[str]:
        return {provider.prefix for provider in self.writable_providers() if provider.prefix}

    def submodule_paths(self) -> list[str]:
        paths: list[str] = []
        for provider in self.providers:
            if provider.use_submodule:
                for rel_path in (
                    provider.repo_path,
                    provider.symbols_path,
                    provider.footprints_path,
                    provider.models3d_path,
                    provider.design_blocks_path,
                    provider.database_path,
                ):
                    rel = _normalize_rel(rel_path)
                    if rel and rel != "." and rel not in paths:
                        paths.append(rel)
        # Keep the parser utility submodule in the managed list.
        utility_rel = "libs/kicad-library-utils"
        if utility_rel not in paths:
            paths.append(utility_rel)
        return paths


def config_path(workspace_root: Path) -> Path:
    return workspace_root / CONFIG_FILE


def example_config_path(workspace_root: Path) -> Path:
    return workspace_root / EXAMPLE_CONFIG_FILE


def _normalize_rel(path: Path) -> str:
    return str(path).replace("\\", "/").strip("/")


def _default_kicad_provider() -> Provider:
    return Provider(
        id="kicad",
        display_name="KiCad Reference (read-only)",
        prefix="",
        visibility="public",
        priority=100,
        symbols_path=Path("libs/kicad-symbols"),
        footprints_path=Path("libs/kicad-footprints"),
        models3d_path=Path("libs/kicad-packages3D"),
        repo_url="https://gitlab.com/kicad/libraries/kicad-symbols.git",
        repo_path=Path("libs/kicad-symbols"),
        use_submodule=True,
        source="kicad",
    )


def _provider_from_dict(data: dict, fallback_id: str) -> Provider:
    provider_id = str(data.get("id") or fallback_id).strip().lower().replace(" ", "-")
    prefix = str(data.get("prefix") or "").strip().upper()
    return Provider(
        id=provider_id,
        display_name=str(data.get("display_name") or provider_id).strip(),
        prefix=prefix,
        visibility=str(data.get("visibility") or "private").strip().lower(),
        priority=int(data.get("priority", 0)),
        symbols_path=Path(str(data.get("symbols_path") or "")),
        footprints_path=Path(str(data.get("footprints_path") or "")),
        models3d_path=Path(str(data.get("models3d_path") or "")),
        design_blocks_path=Path(str(data.get("design_blocks_path") or "")),
        database_path=Path(str(data.get("database_path") or "")),
        repo_url=str(data.get("repo_url") or "").strip(),
        repo_path=Path(str(data.get("repo_path") or ".")),
        auth_method_last_success=str(data.get("auth_method_last_success") or "").strip(),
        last_verified_at=str(data.get("last_verified_at") or "").strip(),
        use_submodule=bool(data.get("use_submodule", False)),
        source=str(data.get("source") or "provider").strip().lower(),
    )


def _provider_to_dict(provider: Provider) -> dict[str, object]:
    return {
        "id": provider.id,
        "display_name": provider.display_name,
        "prefix": provider.prefix,
        "visibility": provider.visibility,
        "priority": provider.priority,
        "repo_url": provider.repo_url,
        "repo_path": _normalize_rel(provider.repo_path),
        "symbols_path": _normalize_rel(provider.symbols_path),
        "footprints_path": _normalize_rel(provider.footprints_path),
        "models3d_path": _normalize_rel(provider.models3d_path),
        "design_blocks_path": _normalize_rel(provider.design_blocks_path),
        "database_path": _normalize_rel(provider.database_path),
        "auth_method_last_success": provider.auth_method_last_success,
        "last_verified_at": provider.last_verified_at,
        "use_submodule": provider.use_submodule,
        "source": provider.source,
    }


def _load_json_like(path: Path) -> dict:
    text = path.read_text(encoding="utf-8").strip()
    if not text:
        return {}
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        try:
            import yaml  # type: ignore
        except Exception as exc:
            raise ValueError(
                f"{path.name} is not valid JSON and PyYAML is not installed for YAML parsing."
            ) from exc
        loaded = yaml.safe_load(text) or {}
        if not isinstance(loaded, dict):
            raise ValueError(f"{path.name} must contain a mapping at the top level.")
        return loaded


def legacy_provider_registry() -> ProviderRegistry:
    providers = [_default_kicad_provider()]
    providers.sort(key=lambda provider: provider.priority, reverse=True)
    return ProviderRegistry(providers=providers, source_name="legacy-defaults")


def _validate_prefixes(providers: list[Provider]) -> None:
    pattern = re.compile(r"^[A-Z]{2,3}$")
    seen: dict[str, str] = {}
    for provider in providers:
        if not provider.has_parts():
            continue
        if not pattern.match(provider.prefix):
            raise ValueError(
                f"Provider '{provider.id}' has invalid prefix '{provider.prefix}'. Expected 2-3 uppercase letters."
            )
        if provider.prefix in seen:
            raise ValueError(
                f"Provider prefix '{provider.prefix}' is duplicated by '{seen[provider.prefix]}' and '{provider.id}'."
            )
        seen[provider.prefix] = provider.id


def load_provider_registry(workspace_root: Path) -> ProviderRegistry:
    local_path = config_path(workspace_root)
    if local_path.exists():
        loaded = _load_json_like(local_path)
        providers = [_provider_from_dict(entry, f"provider-{idx}") for idx, entry in enumerate(loaded.get("providers", []), start=1)]
        providers = [provider for provider in providers if provider.source != "local"]
        _validate_prefixes(providers)
        providers.sort(key=lambda provider: provider.priority, reverse=True)
        return ProviderRegistry(providers=providers, source_name=CONFIG_FILE)
    return legacy_provider_registry()


def save_provider_registry(workspace_root: Path, providers: list[Provider]) -> Path:
    serializable = {
        "version": CONFIG_VERSION,
        "providers": [_provider_to_dict(provider) for provider in providers],
    }
    target = config_path(workspace_root)
    target.write_text(json.dumps(serializable, indent=2) + "\n", encoding="utf-8")
    return target


def write_example_config(workspace_root: Path) -> Path:
    target = example_config_path(workspace_root)
    if target.exists():
        return target
    defaults = legacy_provider_registry().providers
    payload = {
        "version": CONFIG_VERSION,
        "providers": [_provider_to_dict(provider) for provider in defaults],
        "_comment": "Copy this file to library-providers.yaml and edit local mappings.",
    }
    target.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return target


def bootstrap_local_provider_config(workspace_root: Path) -> Path:
    target = config_path(workspace_root)
    if target.exists():
        return target
    providers = legacy_provider_registry().providers
    return save_provider_registry(workspace_root, providers)


def with_verified_auth(provider: Provider, auth_method: str) -> Provider:
    stamp = datetime.now(timezone.utc).isoformat(timespec="seconds").replace("+00:00", "Z")
    return Provider(
        id=provider.id,
        display_name=provider.display_name,
        prefix=provider.prefix,
        visibility=provider.visibility,
        priority=provider.priority,
        symbols_path=provider.symbols_path,
        footprints_path=provider.footprints_path,
        models3d_path=provider.models3d_path,
        design_blocks_path=provider.design_blocks_path,
        database_path=provider.database_path,
        repo_url=provider.repo_url,
        repo_path=provider.repo_path,
        auth_method_last_success=auth_method,
        last_verified_at=stamp,
        use_submodule=provider.use_submodule,
        source=provider.source,
    )
