# main.py
# SPDX-FileCopyrightText: 2023 nate-xyz
# SPDX-License-Identifier: GPL-3.0-or-later

from importer import Importer
from loguru import logger

logger.debug("in python main")
importer = Importer()

def tags_and_cover_art(directory_path):
    logger.debug(f"retrieving tags & cover art from: {directory_path}")
    BOTH_HASHMAP = importer.load_folder_both(directory_path)
    return BOTH_HASHMAP

def tags(directory_path) -> dict:
    logger.debug(f"retrieving tags from: {directory_path}")
    TAGS_HASHMAP = importer.load_folder(directory_path)
    return TAGS_HASHMAP

def cover_art(directory_path) -> dict:
    logger.debug(f"retrieving cover art from: {directory_path}")
    COVER_ART_HASHMAP = importer.load_folder_coverart(directory_path)
    return COVER_ART_HASHMAP

def tags_and_cover_art_from_files(filepaths):
    """Read only the legacy formats which are not handled by the native Rust reader."""
    logger.debug(f"retrieving legacy tags & cover art for {len(filepaths)} files")
    return importer.get_both(filepaths)
