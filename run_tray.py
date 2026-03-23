#!/usr/bin/env python
"""
Run the Audio-to-Text Tray Tool
"""
import sys
from pathlib import Path

# Add project root to path
project_root = Path(__file__).parent
sys.path.insert(0, str(project_root))

from tray.main import main

if __name__ == "__main__":
    main()
