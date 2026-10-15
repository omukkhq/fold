import { configure } from "@testing-library/react";

// Checks run beside parallel builds on one machine; every wait ends on a condition, so a longer cap hides no sleep.
configure({ asyncUtilTimeout: 5000 });
