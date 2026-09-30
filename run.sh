#!/bin/bash

# ANSI color codes
RED='\033[91m'
GREEN='\033[92m'
YELLOW='\033[93m'
BLUE='\033[94m'
CYAN='\033[96m'
RESET='\033[0m'

# Run script for Discord Honeypot Bot
# This script ensures the virtual environment is set up and runs the bot

# Check if .venv exists
if [ ! -d ".venv" ]; then
    echo -e "${CYAN}Creating virtual environment...${RESET}"
    python3 -m venv .venv
fi

# Activate virtual environment
echo -e "${CYAN}Activating virtual environment...${RESET}"
source .venv/bin/activate

# Check if requirements.txt exists and install dependencies if not already installed
if [ -f "requirements.txt" ]; then
    # Check if all requirements are already satisfied
    if pip check > /dev/null 2>&1; then
        echo -e "${GREEN}Dependencies already installed.${RESET}"
    else
        echo -e "${YELLOW}Installing missing dependencies...${RESET}"
        pip install -r requirements.txt
    fi
fi

# Run the bot
echo -e "${GREEN}Starting bot...${RESET}"
python bot.py
