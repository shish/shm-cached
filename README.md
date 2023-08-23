shm-cached
==========
A caching reverse-proxy specifically optimised for [Shimmie](https://github.com/shish/shimmie2/) galleries

- Designed to run as a fleet of image mirrors who use consistent hashing to optimally spread the load
- Reads the load balancing weights directly from Shimmie, no local config to manage
- Listens for config changes in real time - the fleet of mirrors will automatically redirect traffic between themselves when the load balancing weights change
- Integrates with the Image Bans system to automatically purge things from the cache whenever an image is banned from the gallery

Why
---
- When you have 100GB of images, and 4 x 32GB caches, this allows
  each host to be responsible for 25GB of data (100% cache hit rate)
  instead of random / round-robin balacing (~30% cache hit rate)
- The algorithm allows each image to be owned by a primary and
  secondary cache (so each host would be primary for 25GB and
  secondary for 25GB) - this means that if a host has a hardware
  failure, it can be pulled out, and the secondary hosts will be
  warmed up already (even if they need to go to disk instead of RAM)
- When the balancing config changes, hosts redirect load to the
  new owner instead of lowering their own cache hit rate

How to use
----------
- Run this software on the three hosts `mirror-{a,b,c}.mysite.com`
- Set `image_ilink` to something like:
  `https://mirror-{a=5,b=5,c=10}.mysite.com/_images/$hash/$id-$tags.$ext`
- Shimmie then generates links like
  `https://mirror-c.mysite.com/_images/ab25bc2/42-tagme.jpg`
- If the user visits the above URL, then
  - `mirror-c` checks to see which mirror is responsible for the file
    according to the load balancing algorithm. If somebody else is the
    current owner (eg the user is following an old link from google),
    it sends an HTTP redirect to that host
  - If the server is responsible for a file but doesn't have a copy, it
    fetches from the backend source of truth
  - The server sends the file to the client
