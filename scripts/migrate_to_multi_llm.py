#!/usr/bin/env python3
"""
Migration Script: Single DeepSeek to Multi-LLM Configuration

Migrates existing DeepSeek API key from .env to the new multi-LLM
configuration system.

Usage:
    python scripts/migrate_to_multi_llm.py [--dry-run]
"""
import os
import sys
import json
from pathlib import Path
from datetime import datetime

# Add project root to path
sys.path.insert(0, str(Path(__file__).parent.parent))

from app.utils.encryption import get_encryption
from app.llm.models import LLMProvider, LLMConfig
from app.config import settings


def read_env_file(env_path: str = ".env") -> dict:
    """Read .env file and return key-value pairs."""
    env_vars = {}

    if not os.path.exists(env_path):
        return env_vars

    with open(env_path, "r") as f:
        for line in f:
            line = line.strip()
            if not line or line.startswith("#"):
                continue

            if "=" in line:
                key, value = line.split("=", 1)
                env_vars[key.strip()] = value.strip().strip('"').strip("'")

    return env_vars


def migrate_deepseek_config(dry_run: bool = False) -> bool:
    """
    Migrate DeepSeek configuration from .env to multi-LLM system.

    Args:
        dry_run: If True, only print what would be done without making changes

    Returns:
        True if migration successful or no migration needed
    """
    data_dir = Path("data")
    llm_configs_file = data_dir / "llm_configs.json"

    # Check if already migrated
    if llm_configs_file.exists():
        try:
            with open(llm_configs_file, "r") as f:
                data = json.load(f)

            if data.get("configs"):
                print("✓ LLM configs already exist, skipping migration")
                print(f"  Found {len(data['configs'])} existing configuration(s)")
                return True
        except Exception as e:
            print(f"! Error reading existing configs: {e}")

    # Read .env file
    env_vars = read_env_file()

    # Get DeepSeek API key
    api_key = env_vars.get("DEEPSEEK_API_KEY") or os.getenv("DEEPSEEK_API_KEY")

    if not api_key:
        print("! No DEEPSEEK_API_KEY found in .env or environment")
        print("  Skipping migration - you can add LLM configs manually via the UI")
        return True

    print(f"Found DEEPSEEK_API_KEY: {api_key[:8]}...{api_key[-4:]}")

    # Get other DeepSeek settings
    model = env_vars.get("DEEPSEEK_MODEL") or settings.deepseek_model
    base_url = env_vars.get("DEEPSEEK_BASE_URL") or settings.deepseek_base_url
    max_tokens = int(env_vars.get("DEEPSEEK_MAX_TOKENS", 4096))
    temperature = float(env_vars.get("DEEPSEEK_TEMPERATURE", 0.3))

    if dry_run:
        print("\n[DRY RUN] Would create the following configuration:")
        print(f"  Provider: DeepSeek")
        print(f"  Model: {model}")
        print(f"  Base URL: {base_url}")
        print(f"  Max Tokens: {max_tokens}")
        print(f"  Temperature: {temperature}")
        print(f"  Config File: {llm_configs_file}")
        return True

    # Ensure data directory exists
    data_dir.mkdir(parents=True, exist_ok=True)

    # Encrypt API key
    encryption = get_encryption()
    encrypted_key = encryption.encrypt(api_key)

    # Create LLM config
    config_id = f"llm-{int(datetime.now().timestamp() * 1000)}"

    config_data = {
        "id": config_id,
        "provider": LLMProvider.DEEPSEEK.value,
        "name": "DeepSeek (迁移)",
        "api_key_encrypted": encrypted_key,
        "api_base": base_url,
        "model": model,
        "temperature": temperature,
        "max_tokens": max_tokens,
        "extra_params": {},
        "is_default": True,
        "created_at": datetime.now().isoformat(),
        "updated_at": None
    }

    # Save config
    data = {
        "configs": [config_data],
        "default_config_id": config_id
    }

    try:
        with open(llm_configs_file, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)

        print(f"\n✓ Successfully migrated DeepSeek configuration")
        print(f"  Config ID: {config_id}")
        print(f"  Config file: {llm_configs_file}")
        print(f"\nYou can now manage LLM configurations via Settings → LLM Services")

        return True

    except Exception as e:
        print(f"✗ Failed to save configuration: {e}")
        return False


def main():
    """Main entry point."""
    dry_run = "--dry-run" in sys.argv

    print("=" * 50)
    print("Migrating to Multi-LLM Configuration")
    print("=" * 50)

    if dry_run:
        print("[DRY RUN MODE - No changes will be made]\n")

    success = migrate_deepseek_config(dry_run)

    if success:
        print("\n" + "=" * 50)
        print("Migration completed successfully")
        print("=" * 50)
        sys.exit(0)
    else:
        print("\n" + "=" * 50)
        print("Migration failed")
        print("=" * 50)
        sys.exit(1)


if __name__ == "__main__":
    main()
