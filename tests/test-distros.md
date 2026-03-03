chmod +x test-portable.sh test-install.sh

# Fast test — binary works?
sudo ./test-portable.sh

# Full test — install experience works?
sudo ./test-install.sh

# Debug one distro
sudo ./test-portable.sh fedora --verbose
sudo ./test-install.sh amazon-linux --verbose
