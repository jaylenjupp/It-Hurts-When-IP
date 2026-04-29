#!/bin/bash

echo "Uninstalling It Hurts When IP..."

# Unload and remove helper
if launchctl list | grep -q "com.ipswitcher.helper"; then
    sudo launchctl unload /Library/LaunchDaemons/com.ipswitcher.helper.plist 2>/dev/null
fi

sudo rm -f /Library/PrivilegedHelperTools/com.ipswitcher.helper
sudo rm -f /Library/LaunchDaemons/com.ipswitcher.helper.plist

# Remove app from Applications if it exists
if [ -d "/Applications/It Hurts When IP.app" ]; then
    sudo rm -rf "/Applications/It Hurts When IP.app"
    echo "App removed from Applications folder"
fi

# Remove app data
rm -rf ~/Library/Application\ Support/com.ipswitcher.switcher

echo "Uninstall complete."