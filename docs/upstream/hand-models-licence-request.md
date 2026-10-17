# Draft: ask the Monado maintainers for a licence on the hand-tracking-models repository

Status: DRAFT, not sent. To be reviewed and sent by the project owner (for example as an issue on
`gitlab.freedesktop.org/monado/utilities/hand-tracking-models`, or a message to the Monado maintainers).

---

**Subject:** Licence for the hand-tracking-models repository

Hello Monado maintainers,

Thank you for Monado and for the Mercury hand tracking. I use it on Linux with a Pimax Dream Air headset and it works
well.

I noticed that `monado/utilities/hand-tracking-models` has no LICENSE file, and GitLab shows no licence for it. Because
of that I treat the models as "all rights reserved": my project (omarchyvr, a Linux app for that headset) does not
bundle or redistribute them. Instead, the app asks the user to download them from your public server onto their own
PC, for personal use, after showing a notice that they have no licence.

It would help a lot of people if the repository stated its terms. Could you please consider adding a licence file, or
tell us what use is intended (personal use, redistribution in distro packages, use in other runtimes)? If there is a
reason a licence cannot be added, a short note in the README saying so would also help packagers and users.

With a clear licence, distributions could package the models, and projects like mine could stop asking users to fetch
them by hand.

Thank you for your time, and thanks again for the work on Monado.

Kind regards,
the omarchyvr maintainer
